"""Stand-ins shared by the tests."""


class SilentEndpoint:
    """An audio endpoint that never hands over a packet, like loopback while nothing plays."""

    def __enter__(self):
        return self

    def __exit__(self, *exc_info) -> None:
        pass

    def read(self) -> list:
        return []
