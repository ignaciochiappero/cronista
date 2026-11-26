//! Transcript exports: SRT, plain text, pure-Rust Word (.docx OOXML), and pure-Rust PDF
//! (with embedded Windows TrueType Segoe UI / Arial fonts so accents, ñ, ¿¡, and → render everywhere).

use crate::engine::transcript::{to_srt, TranscriptionSegment};
use crate::engine::voices::line;
use crate::language::export_words;
use chrono::NaiveDateTime;
use printpdf::{BuiltinFont, Color, IndirectFontRef, Mm, PdfDocument, PdfLayerReference, Rgb};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}")
}

fn time_range(seg: &TranscriptionSegment) -> String {
    format!("[{} → {}]", clock(seg.start), clock(seg.end))
}

fn reading_paragraphs(segments: &[TranscriptionSegment]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut buffer: Vec<&str> = Vec::new();
    let mut length = 0usize;
    let joined: String = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    for word in joined.split_whitespace() {
        buffer.push(word);
        length += word.len() + 1;
        if length > 500
            && (word.ends_with('.')
                || word.ends_with('?')
                || word.ends_with('!')
                || word.ends_with(':'))
        {
            paragraphs.push(buffer.join(" "));
            buffer.clear();
            length = 0;
        }
    }
    if !buffer.is_empty() {
        paragraphs.push(buffer.join(" "));
    }
    paragraphs
}

fn turns(
    segments: &[TranscriptionSegment],
    names: Option<&HashMap<String, String>>,
) -> Vec<(Option<String>, String)> {
    let Some(names) = names else {
        return reading_paragraphs(segments)
            .into_iter()
            .map(|p| (None, p))
            .collect();
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < segments.len() {
        let src = segments[i].source.as_deref();
        let mut j = i + 1;
        while j < segments.len() && segments[j].source.as_deref() == src {
            j += 1;
        }
        let speaker_name = src.and_then(|s| names.get(s)).cloned();
        for (idx, para) in reading_paragraphs(&segments[i..j]).into_iter().enumerate() {
            out.push((if idx == 0 { speaker_name.clone() } else { None }, para));
        }
        i = j;
    }
    out
}

pub fn write_text_files(
    segments: &[TranscriptionSegment],
    folder: &Path,
    names: Option<&HashMap<String, String>>,
) -> Result<(PathBuf, PathBuf), String> {
    fs::create_dir_all(folder).map_err(|e| format!("create export dir failed: {e}"))?;
    let srt = folder.join("transcript.srt");
    let txt = folder.join("transcript.txt");
    fs::write(&srt, to_srt(segments, names)).map_err(|e| format!("write srt failed: {e}"))?;
    let mut txt_content = String::new();
    for seg in segments {
        txt_content.push_str(&line(seg, names));
        txt_content.push('\n');
    }
    fs::write(&txt, txt_content).map_err(|e| format!("write txt failed: {e}"))?;
    Ok((srt, txt))
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Pure-Rust Word (.docx) generator matching Cronista's `write_word` structure & styling.
pub fn write_word(
    segments: &[TranscriptionSegment],
    output_path: &Path,
    title: &str,
    recorded_at: NaiveDateTime,
    language: Option<&str>,
    names: Option<&HashMap<String, String>>,
    from_file: bool,
) -> Result<PathBuf, String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create docx dir failed: {e}"))?;
    }
    let words = export_words(language);
    let date_line = words.format_date(recorded_at, from_file);

    let mut body = String::with_capacity(4096);
    // Title (Heading 1 centered)
    body.push_str(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        xml_escape(title)
    ));
    // Recorded / Transcribed date (centered, 10pt, grey)
    body.push_str(&format!(
        r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:color w:val="646464"/><w:sz w:val="20"/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        xml_escape(&date_line)
    ));

    // Transcript heading (Heading 2)
    body.push_str(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        xml_escape(words.transcript)
    ));
    for (speaker, text) in turns(segments, names) {
        body.push_str("<w:p>");
        if let Some(sp) = speaker {
            body.push_str(&format!(
                r#"<w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">{}: </w:t></w:r>"#,
                xml_escape(&sp)
            ));
        }
        body.push_str(&format!(
            r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
            xml_escape(&text)
        ));
    }

    // Timestamps heading (Heading 2)
    body.push_str(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        xml_escape(words.timestamps)
    ));
    for seg in segments {
        body.push_str(r#"<w:p><w:pPr><w:spacing w:after="40"/></w:pPr>"#);
        body.push_str(&format!(
            r#"<w:r><w:rPr><w:b/><w:color w:val="464646"/><w:sz w:val="18"/></w:rPr><w:t xml:space="preserve">{} </w:t></w:r>"#,
            xml_escape(&time_range(seg))
        ));
        if let (Some(map), Some(src)) = (names, seg.source.as_deref()) {
            if let Some(sp) = map.get(src) {
                body.push_str(&format!(
                    r#"<w:r><w:rPr><w:b/><w:sz w:val="20"/></w:rPr><w:t xml:space="preserve">{}: </w:t></w:r>"#,
                    xml_escape(sp)
                ));
            }
        }
        body.push_str(&format!(
            r#"<w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
            xml_escape(&seg.text)
        ));
    }

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body>
</w:document>"#
    );

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
    <w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/><w:sz w:val="22"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="240" w:after="120"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="36"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading2">
    <w:name w:val="heading 2"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="200" w:after="80"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="26"/></w:rPr>
  </w:style>
</w:styles>"#;

    let file = File::create(output_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "{} is open in another program. Close it and try again.",
                output_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("transcript.docx")
            )
        } else {
            format!("create docx failed: {e}")
        }
    })?;

    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels),
        ("word/_rels/document.xml.rels", doc_rels),
        ("word/styles.xml", styles),
        ("word/document.xml", document_xml.as_str()),
    ] {
        zip.start_file(name, opts)
            .map_err(|e| format!("zip entry {name} failed: {e}"))?;
        zip.write_all(data.as_bytes())
            .map_err(|e| format!("zip write {name} failed: {e}"))?;
    }
    zip.finish().map_err(|e| format!("zip finish failed: {e}"))?;
    Ok(output_path.to_path_buf())
}

fn find_system_ttf_pair() -> Option<(PathBuf, PathBuf)> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let candidates = [
        (
            PathBuf::from(&windir).join("Fonts").join("segoeui.ttf"),
            PathBuf::from(&windir).join("Fonts").join("segoeuib.ttf"),
        ),
        (
            PathBuf::from(&windir).join("Fonts").join("arial.ttf"),
            PathBuf::from(&windir).join("Fonts").join("arialbd.ttf"),
        ),
        (
            PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
            PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
        ),
    ];
    for (reg, bold) in candidates {
        if reg.is_file() && bold.is_file() {
            return Some((reg, bold));
        }
    }
    None
}

fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let cur_len = current.chars().count();
        let word_len = word.chars().count();
        if !current.is_empty() && cur_len + 1 + word_len > max_chars {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Pure-Rust PDF generator with embedded Windows TrueType font (Segoe UI / Arial -> `/FontFile2`),
/// automatic pagination on A4, centered title/date, turns by speaker, timestamps, and page numbers.
pub fn write_pdf(
    segments: &[TranscriptionSegment],
    output_path: &Path,
    title: &str,
    recorded_at: NaiveDateTime,
    language: Option<&str>,
    names: Option<&HashMap<String, String>>,
    from_file: bool,
) -> Result<PathBuf, String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create pdf dir failed: {e}"))?;
    }

    let page_w = Mm(210.0);
    let page_h = Mm(297.0);
    let margin_x = 22.0_f32;
    let margin_top = 20.0_f32;
    let margin_bottom = 22.0_f32;

    let (doc, page1, layer1) = PdfDocument::new(title, page_w, page_h, "Layer 1");

    let (font_reg, font_bold): (IndirectFontRef, IndirectFontRef) =
        if let Some((reg_p, bold_p)) = find_system_ttf_pair() {
            let reg_bytes = fs::read(&reg_p).map_err(|e| format!("read ttf failed: {e}"))?;
            let bold_bytes = fs::read(&bold_p).map_err(|e| format!("read bold ttf failed: {e}"))?;
            let r = doc
                .add_external_font(Cursor::new(reg_bytes))
                .map_err(|e| format!("embed regular font failed: {e}"))?;
            let b = doc
                .add_external_font(Cursor::new(bold_bytes))
                .map_err(|e| format!("embed bold font failed: {e}"))?;
            (r, b)
        } else {
            let r = doc
                .add_builtin_font(BuiltinFont::Helvetica)
                .map_err(|e| format!("builtin font failed: {e}"))?;
            let b = doc
                .add_builtin_font(BuiltinFont::HelveticaBold)
                .map_err(|e| format!("builtin bold failed: {e}"))?;
            (r, b)
        };

    let words = export_words(language);
    let date_line = words.format_date(recorded_at, from_file);

    struct PdfCursor<'a> {
        doc: &'a printpdf::PdfDocumentReference,
        layer: PdfLayerReference,
        y: f32,
        page_num: usize,
        page_w: f32,
        page_h: f32,
        margin_x: f32,
        margin_top: f32,
        margin_bottom: f32,
        font_reg: &'a IndirectFontRef,
    }

    impl<'a> PdfCursor<'a> {
        fn draw_page_number(&self) {
            self.layer
                .set_fill_color(Color::Rgb(Rgb::new(0.54, 0.54, 0.54, None)));
            self.layer.use_text(
                self.page_num.to_string(),
                8.0,
                Mm(self.page_w - self.margin_x - 6.0),
                Mm(12.0),
                self.font_reg,
            );
            self.layer
                .set_fill_color(Color::Rgb(Rgb::new(0.08, 0.08, 0.1, None)));
        }

        fn ensure_space(&mut self, needed_mm: f32) {
            if self.y - needed_mm < self.margin_bottom {
                let (p, l) = self.doc.add_page(Mm(self.page_w), Mm(self.page_h), "Layer 1");
                self.layer = self.doc.get_page(p).get_layer(l);
                self.page_num += 1;
                self.y = self.page_h - self.margin_top;
                self.draw_page_number();
            }
        }

        fn draw_line(&mut self, text: &str, size_pt: f32, leading_mm: f32, font: &IndirectFontRef, color: (f32, f32, f32)) {
            self.ensure_space(leading_mm);
            self.layer
                .set_fill_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));
            self.layer
                .use_text(text, size_pt, Mm(self.margin_x), Mm(self.y), font);
            self.y -= leading_mm;
        }
    }

    let first_layer = doc.get_page(page1).get_layer(layer1);
    let mut cur = PdfCursor {
        doc: &doc,
        layer: first_layer,
        y: page_h.0 - margin_top,
        page_num: 1,
        page_w: page_w.0,
        page_h: page_h.0,
        margin_x,
        margin_top,
        margin_bottom,
        font_reg: &font_reg,
    };
    cur.draw_page_number();

    // Title & date
    for line in wrap_text(title, 52) {
        cur.draw_line(&line, 17.0, 7.5, &font_bold, (0.07, 0.07, 0.09));
    }
    cur.draw_line(&date_line, 10.0, 5.5, &font_reg, (0.39, 0.39, 0.39));
    cur.y -= 4.0;

    // Transcript section
    cur.draw_line(words.transcript, 13.0, 6.5, &font_bold, (0.07, 0.07, 0.09));
    cur.y -= 1.0;
    for (speaker, text) in turns(segments, names) {
        let full = if let Some(sp) = speaker {
            format!("{sp}: {text}")
        } else {
            text
        };
        for l in wrap_text(&full, 82) {
            cur.draw_line(&l, 10.5, 5.2, &font_reg, (0.08, 0.08, 0.1));
        }
        cur.y -= 2.2;
    }

    // Timestamps section
    cur.y -= 2.5;
    cur.draw_line(words.timestamps, 13.0, 6.5, &font_bold, (0.07, 0.07, 0.09));
    cur.y -= 1.0;
    for seg in segments {
        let prefix = if let (Some(map), Some(src)) = (names, seg.source.as_deref()) {
            if let Some(sp) = map.get(src) {
                format!("{} {sp}: {}", time_range(seg), seg.text)
            } else {
                format!("{} {}", time_range(seg), seg.text)
            }
        } else {
            format!("{} {}", time_range(seg), seg.text)
        };
        for l in wrap_text(&prefix, 88) {
            cur.draw_line(&l, 9.5, 4.6, &font_reg, (0.15, 0.15, 0.18));
        }
        cur.y -= 1.0;
    }

    let file = File::create(output_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "{} is open in another program. Close it and try again.",
                output_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("transcript.pdf")
            )
        } else {
            format!("create pdf failed: {e}")
        }
    })?;
    let mut writer = BufWriter::new(file);
    doc.save(&mut writer)
        .map_err(|e| format!("save pdf failed: {e}"))?;
    Ok(output_path.to_path_buf())
}
