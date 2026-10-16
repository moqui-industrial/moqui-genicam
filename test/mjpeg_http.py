"""HTTP MJPEG checks against an explicitly configured mock-camera application URL."""
import argparse
import time
from urllib.request import Request, urlopen
from urllib.parse import urlencode


def frame(response):
    boundary = response.readline()
    assert boundary == b"--moqui-frame\r\n", boundary
    headers = {}
    while True:
        line = response.readline()
        if line == b"\r\n":
            break
        key, value = line.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    image = response.read(int(headers["content-length"]))
    assert headers["content-type"] == "image/jpeg"
    assert image.startswith(b"\xff\xd8") and image.endswith(b"\xff\xd9")
    assert response.read(2) == b"\r\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True, help="Mounted GenicamStream/stream URL")
    parser.add_argument("--cookie", help="Authenticated test session cookie when required")
    args = parser.parse_args()
    def request(count):
        url = args.url + ("&" if "?" in args.url else "?") + urlencode({"deviceId":"FLIR_CAMERA_1","maxFrames":count,"frameDelayMs":700})
        headers = {"Cookie":args.cookie} if args.cookie else {}
        return urlopen(Request(url,headers=headers),timeout=15)
    with request(2) as response:
        assert response.headers["Content-Type"] == "multipart/x-mixed-replace; boundary=moqui-frame"
        assert response.headers.get("Content-Length") is None
        frame(response)
        first = time.monotonic()
        frame(response)
        assert time.monotonic() - first > 0.3, "frames must arrive progressively"
        assert response.read() == b"--moqui-frame--\r\n"
    with request(0) as response:
        frame(response)
        # Closing an unlimited body must not retain an unbounded capture queue.
    with request(1) as response:
        frame(response)
        assert response.read() == b"--moqui-frame--\r\n"
    print("MJPEG: progressive JPEG parts, finite close, unlimited disconnect and subsequent capture passed")


if __name__ == "__main__":
    main()
