#!/usr/bin/env python3
"""Signs the installers with the Sparkle EdDSA key and prints a WinSparkle appcast.

    scripts/appcast.py <version> <x64 installer> <x64 url> <arm64 installer> <arm64 url> [notes.md] > appcast.xml
    scripts/appcast.py --check        # only checks that the private key matches the public key in the app

The private key (base64 of the 32-byte Ed25519 seed, the same one the macOS version's Sparkle feed uses) comes
from SPARKLE_ED_PRIVATE_KEY, else ~/.shotlate-signing/sparkle_ed25519_private.txt. WinSparkle compares
<sparkle:version> with the app's version and picks the enclosure whose sparkle:os matches its architecture.
Needs `pip install cryptography`.
"""
import base64
import email.utils
import html
import os
import pathlib
import re
import sys

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

ROOT = pathlib.Path(__file__).resolve().parent.parent


def private_key() -> Ed25519PrivateKey:
    raw = os.environ.get("SPARKLE_ED_PRIVATE_KEY")
    if not raw:
        path = pathlib.Path.home() / ".shotlate-signing" / "sparkle_ed25519_private.txt"
        if not path.exists():
            sys.exit("No EdDSA key: set SPARKLE_ED_PRIVATE_KEY or create ~/.shotlate-signing/sparkle_ed25519_private.txt")
        raw = path.read_text()
    seed = base64.b64decode(raw.strip())
    if len(seed) != 32:
        sys.exit(f"The EdDSA key should be a 32-byte seed, got {len(seed)} bytes")
    return Ed25519PrivateKey.from_private_bytes(seed)


def app_public_key() -> str:
    source = (ROOT / "src" / "win" / "updater.rs").read_text()
    m = re.search(r'EDDSA_PUBLIC_KEY: &str = "([^"]+)"', source)
    if not m:
        sys.exit("EDDSA_PUBLIC_KEY not found in src/win/updater.rs")
    return m.group(1)


def check(key: Ed25519PrivateKey) -> None:
    public = base64.b64encode(key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)).decode()
    if public != app_public_key():
        sys.exit("The private key doesn't match EDDSA_PUBLIC_KEY in src/win/updater.rs; installed copies would reject the update")


def enclosure(key: Ed25519PrivateKey, path: str, url: str, os_name: str) -> str:
    data = pathlib.Path(path).read_bytes()
    signature = base64.b64encode(key.sign(data)).decode()
    return (
        f'<enclosure url="{html.escape(url)}" sparkle:os="{os_name}" length="{len(data)}" type="application/octet-stream"\n'
        f'                 sparkle:edSignature="{signature}"\n'
        f'                 sparkle:installerArguments="/SILENT /SP- /NOICONS /SUPPRESSMSGBOXES" />'
    )


def main() -> None:
    key = private_key()
    check(key)
    if sys.argv[1:] == ["--check"]:
        print("EdDSA key matches the app")
        return
    if len(sys.argv) < 6:
        sys.exit(__doc__)
    version, x64, x64_url, arm64, arm64_url = sys.argv[1:6]
    notes = pathlib.Path(sys.argv[6]).read_text() if len(sys.argv) > 6 else ""
    description = f"<description><![CDATA[{notes.replace(']]>', ']]]]><![CDATA[>')}]]></description>" if notes else ""
    items = []
    for path, url, os_name in [(x64, x64_url, "windows-x64"), (arm64, arm64_url, "windows-arm64")]:
        if path and os.path.exists(path):
            items.append(
                f"""    <item>
      <title>Shotlate {version}</title>
      <pubDate>{email.utils.formatdate(usegmt=True)}</pubDate>
      <sparkle:version>{version}</sparkle:version>
      <sparkle:shortVersionString>{version}</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>10.0.19041</sparkle:minimumSystemVersion>
      {description}
      {enclosure(key, path, url, os_name)}
    </item>"""
            )
    print(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">\n'
        "  <channel>\n    <title>Shotlate for Windows</title>\n"
        + "\n".join(items)
        + "\n  </channel>\n</rss>"
    )


if __name__ == "__main__":
    main()
