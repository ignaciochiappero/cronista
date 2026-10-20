from cronista.library import Library, sanitize_fts_query


def add(library, title, transcript, created_at, tmp_path):
    folder = tmp_path / title
    return library.add(
        title=title,
        created_at=created_at,
        duration_seconds=60.0,
        size_bytes=1_000,
        folder=str(folder),
        video_path=str(folder / "recording.mp4"),
        transcript=transcript,
        segments=[{"start": 0.0, "end": 1.0, "text": transcript}],
    )


def test_quotes_every_search_token_so_symbols_are_literal():
    # Same rule as escriba's _sanitize_fts_query, with each token a prefix.
    assert sanitize_fts_query('hola (mundo) "x"') == '"hola"* "(mundo)"* """x"""*'
    assert sanitize_fts_query("   ") is None


def test_lists_newest_first(tmp_path):
    library = Library(tmp_path / "library.db")
    first = add(library, "first", "hola mundo", "2026-10-19T10:00:00", tmp_path)
    second = add(library, "second", "this is english", "2026-10-19T11:00:00", tmp_path)
    assert [r["id"] for r in library.list()] == [second, first]


def test_finds_recordings_by_what_was_said(tmp_path):
    library = Library(tmp_path / "library.db")
    add(library, "demo", "Ahora abro el editor y muestro el proyecto", "2026-10-19T10:00:00", tmp_path)
    add(library, "call", "Let me show you the dashboard", "2026-10-19T11:00:00", tmp_path)
    found = library.list(search="editor")
    assert [r["title"] for r in found] == ["demo"]
    assert "[editor]" in found[0]["snippet"]
    assert [r["title"] for r in library.list(search="dashboard")] == ["call"]


def test_finds_a_word_while_it_is_being_typed(tmp_path):
    library = Library(tmp_path / "library.db")
    add(library, "demo", "Después, precios: el plan anual", "2026-10-19T10:00:00", tmp_path)
    found = library.list(search="prec")
    assert [r["title"] for r in found] == ["demo"]
    assert "[precios]" in found[0]["snippet"]
    assert [r["title"] for r in library.list(search="despues pla")] == ["demo"]  # accents ignored, every word a beginning
    assert library.list(search="recios") == []  # the start of a word, not any part of it


def test_a_search_full_of_symbols_does_not_fail(tmp_path):
    library = Library(tmp_path / "library.db")
    add(library, "demo", "hola", "2026-10-19T10:00:00", tmp_path)
    assert library.list(search='(unbalanced "quote') == []


def test_gets_the_segments_back_and_deletes(tmp_path):
    library = Library(tmp_path / "library.db")
    recording_id = add(library, "demo", "hola", "2026-10-19T10:00:00", tmp_path)
    assert library.get(recording_id)["segments"] == [{"start": 0.0, "end": 1.0, "text": "hola"}]
    assert library.delete(recording_id) is True
    assert library.get(recording_id) is None
    assert library.list(search="hola") == []  # the search index forgets it too
    assert library.delete(recording_id) is False


def test_renames_and_searches_by_the_new_title(tmp_path):
    library = Library(tmp_path / "library.db")
    recording_id = add(library, "untitled", "hola", "2026-10-19T10:00:00", tmp_path)
    library.rename(recording_id, "Sprint review")
    assert library.get(recording_id)["title"] == "Sprint review"
    assert [r["id"] for r in library.list(search="sprint")] == [recording_id]


def test_keeps_each_recordings_labels_and_renames_one(tmp_path):
    library = Library(tmp_path / "library.db")
    recording_id = library.add(title="call", created_at="2026-10-20T10:00:00", duration_seconds=60.0, size_bytes=1,
                               folder=str(tmp_path), video_path=str(tmp_path / "recording.mp4"), transcript="hi",
                               segments=[{"start": 0.0, "end": 1.0, "text": "hi", "source": "computer"}],
                               labels={"mic": "Me", "computer": "Others"})
    assert library.get(recording_id)["labels"] == {"mic": "Me", "computer": "Others"}
    assert library.get(recording_id)["segments"][0]["source"] == "computer"
    library.set_label(recording_id, "computer", "Ana")
    assert library.get(recording_id)["labels"] == {"mic": "Me", "computer": "Ana"}


def test_a_history_from_before_labels_gets_them_without_losing_anything(tmp_path):
    import sqlite3

    from cronista.library import SCHEMA

    path = tmp_path / "library.db"
    with sqlite3.connect(path) as db:  # the tables as versions up to 0.4.0 made them: no labels column
        old_schema = SCHEMA.replace(",\n    labels_json TEXT DEFAULT '{}'", "")
        assert "labels_json" not in old_schema
        db.executescript(old_schema)
        db.execute("INSERT INTO recordings (id, title, created_at, folder, video_path, transcript, segments_json) "
                   "VALUES ('old', 'Old', '2026-10-01T10:00:00', 'f', 'v', 'hola', "
                   "'[{\"start\": 0, \"end\": 1, \"text\": \"hola\"}]')")
    library = Library(path)
    old = library.get("old")
    assert old["labels"] == {} and old["segments"][0]["text"] == "hola"
    library.set_label("old", "mic", "Yo")
    assert library.get("old")["labels"] == {"mic": "Yo"}
