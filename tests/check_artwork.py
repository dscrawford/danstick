"""Does artwork fetching find, name and store the right files?

Runs the whole pipeline against a fixture HTTP server rather than
thumbnails.libretro.com, because the parts that break are the local ones:
the system-name guess, MAME set-name resolution, the escaping of the file
that lands on disk, and resumability. Those need to be checkable on a
machine with no network and without 2 GB of downloads.

The fixture's index pages are real Apache autoindex markup, copied in shape
from the live server, so `parse_index` is exercised against what it will
actually meet.

    python3 tests/check_artwork.py
"""

from __future__ import annotations

import http.server
import os
import shutil
import sys
import tempfile
import threading
import urllib.parse
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import artwork  # noqa: E402
from padmap.titles import Title  # noqa: E402

FAILURES: list[str] = []

# One PNG byte-for-byte: a 1x1 image, so what lands on disk is a real file
# a viewer would open rather than a blob that happens to have the name.
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000d49444154789c6360000002000001e221bc330000000049454e44ae4260"
    "82"
)

# Names as the live server has them: No-Intro for consoles, full MAME
# descriptions for arcade, with libretro's scrub-character escaping applied.
UPSTREAM = {
    "Nintendo - Nintendo 64": [
        "Banjo-Kazooie (USA).png",
        "Banjo-Kazooie (Europe) (En,Fr,De).png",
        "Bomberman 64 (USA).png",
        "Command _ Conquer (USA).png",
    ],
    "MAME": [
        "10-Yard Fight (Japan).png",
        "1941_ Counter Attack (World).png",
        "3 Count Bout _ Fire Suplex.png",
        "PuckMan (harder_).png",
    ],
    "Nintendo - GameCube": [
        "Animal Crossing (USA).png",
    ],
}


def check(name: str, got: object, want: object) -> None:
    if got == want:
        print(f"  ok    {name}")
    else:
        print(f"  FAIL  {name}: got {got!r}, wanted {want!r}")
        FAILURES.append(name)


ROW = (
    '<tr><td valign="top"><img src="/icons/image2.gif" alt="[IMG]"></td>'
    '<td><a href="{href}">{text}</a></td>'
    '<td align="right">2025-04-09 11:50  </td>'
    '<td align="right">{size}</td><td>&nbsp;</td></tr>'
)


def page(entries: list[tuple[str, str]]) -> bytes:
    rows = "\n".join(
        ROW.format(href=urllib.parse.quote(href), text=href, size=size)
        for href, size in entries
    )
    return (
        "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 3.2 Final//EN\">\n"
        "<html><head><title>Index of /</title></head><body>\n"
        '<h1>Index of /</h1><table>\n'
        '<tr><th colspan="5"><hr></th></tr>\n'
        f"{rows}\n"
        "</table></body></html>\n"
    ).encode()


class Fixture(http.server.BaseHTTPRequestHandler):
    # Set per-run so a test can make the server start failing.
    fail_after = -1
    served = 0

    def log_message(self, *_args: object) -> None:
        pass

    def _send(self, body: bytes, kind: str, status: int = 200) -> None:
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        path = urllib.parse.unquote(self.path)
        if path == "/":
            self._send(page([(s + "/", "-") for s in UPSTREAM]), "text/html")
            return

        parts = [p for p in path.split("/") if p]
        if path.endswith("/") and len(parts) == 1:
            self._send(page([(k + "/", "-") for k in artwork.KINDS]), "text/html")
            return
        if path.endswith("/") and len(parts) == 2:
            system, kind = parts
            names = UPSTREAM.get(system, []) if kind == "Named_Boxarts" else []
            self._send(page([(n, "12K") for n in names]), "text/html")
            return

        if len(parts) == 3 and parts[2] in UPSTREAM.get(parts[0], []):
            Fixture.served += 1
            if 0 <= Fixture.fail_after < Fixture.served:
                self._send(b"nope", "text/plain", status=500)
                return
            self._send(PNG, "image/png")
            return
        self._send(b"not found", "text/plain", status=404)


def playlist(path: Path, core: str, items: list[tuple[str, str]]) -> None:
    import json

    path.write_text(json.dumps({
        "version": "1.5",
        "default_core_name": core,
        "items": [
            {"path": rom, "label": label, "db_name": path.name}
            for label, rom in items
        ],
    }))


def main() -> int:
    print("parse_index")
    index = artwork.parse_index(page([("Foo (USA).png", "484K"),
                                      ("Bar.png", "1.2M"),
                                      ("skip.txt", "3K")]).decode())
    check("names", sorted(index), ["Bar", "Foo (USA)"])
    check("size KB", index["Foo (USA)"], 484 * 1024)
    check("size MB", index["Bar"], int(1.2 * 1024 * 1024))

    print("\nsystem resolution")
    available = sorted(UPSTREAM) + ["FBNeo - Arcade Games", "Nintendo - Wii"]
    check("n64 core",
          artwork.resolve_system(
              "Nintendo - Nintendo 64 (Mupen64Plus-Next)", "n64", available),
          "Nintendo - Nintendo 64")
    check("dolphin splits GameCube / Wii",
          artwork.resolve_system(
              "Nintendo - GameCube / Wii (Dolphin)", "gamecube", available),
          "Nintendo - GameCube")
    check("mame core",
          artwork.resolve_system("Arcade (MAME 2010)", "arcade", available),
          "MAME")
    check("fbneo core",
          artwork.resolve_system("Arcade (FinalBurn Neo)", "arcade", available),
          "FBNeo - Arcade Games")
    check("libretro-named playlist",
          artwork.resolve_system("", "Nintendo - Wii", available),
          "Nintendo - Wii")
    check("unknown core",
          artwork.resolve_system("Vectrex (vecx)", "vectrex", available),
          None)

    print("\nmatching")
    names = {n[:-4]: 0 for n in UPSTREAM["Nintendo - Nintendo 64"]}
    by_base = artwork.index_by_base(names)
    check("exact",
          artwork.match_upstream(["Bomberman 64 (USA)"], names, by_base),
          "Bomberman 64 (USA)")
    # GoodN64 label, No-Intro upstream: only the base-title pass can bridge it.
    check("GoodN64 label falls back to base title",
          artwork.match_upstream(["Banjo-Kazooie (U) [!]"], names, by_base),
          "Banjo-Kazooie (USA)")
    check("USA preferred over Europe",
          by_base["banjo-kazooie"], "Banjo-Kazooie (USA)")
    check("no art", artwork.match_upstream(["Nothing At All"], names, by_base),
          None)

    mame = {n[:-4]: 0 for n in UPSTREAM["MAME"]}
    titles = {"10yard": Title("10-Yard Fight (Japan)"),
              "pacman": Title("PuckMan (harder?)")}
    check("MAME set name via title table",
          artwork.match_upstream(
              artwork.candidate_names("10yard", "/g/10yard.zip", titles),
              mame, artwork.index_by_base(mame)),
          "10-Yard Fight (Japan)")
    # The one convention that could silently yield nothing: '?' must be an
    # underscore on both sides of the comparison.
    check("scrub characters survive the round trip",
          artwork.match_upstream(
              artwork.candidate_names("pacman", "/g/pacman.zip", titles),
              mame, artwork.index_by_base(mame)),
          "PuckMan (harder_)")
    check("art_name escapes libretro's set",
          artwork.art_name("A&B*C/D:E`F<G>H?I\\J|K"),
          "A_B_C_D_E_F_G_H_I_J_K")

    print("\nend to end against a fixture server")
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    os.environ[artwork.ENV_SERVER] = f"http://127.0.0.1:{server.server_port}"

    work = Path(tempfile.mkdtemp(prefix="padmap-art-"))
    try:
        playlists = work / "playlists"
        playlists.mkdir()
        dest = work / "thumbnails"
        playlist(playlists / "n64.lpl",
                 "Nintendo - Nintendo 64 (Mupen64Plus-Next)",
                 [("Banjo-Kazooie (U) [!]", "/g/n64/Banjo-Kazooie (U) [!].z64"),
                  ("Bomberman 64 (USA)", "/g/n64/Bomberman 64 (USA).z64"),
                  ("Not A Real Game", "/g/n64/Not A Real Game.z64")])

        plans, error = artwork.plan_all(playlists, dest_root=dest)
        check("no fatal error", error, "")
        plan = plans[0]
        check("system found", plan.system, "Nintendo - Nintendo 64")
        check("matched", plan.matched, 2)
        check("missing is counted, not fatal", plan.missing, 1)
        check("nothing present yet", plan.present, 0)
        check("estimate from index sizes", plan.bytes, 2 * 12 * 1024)

        report = artwork.run_plan(plan, workers=2, progress=False)
        check("downloaded", report.downloaded, 2)
        check("no failures", report.failed, 0)

        boxarts = dest / "n64" / "Named_Boxarts"
        on_disk = sorted(p.name for p in boxarts.iterdir())
        # Stored under the *label*, not the upstream name, because the
        # label is the only key a consumer of the tree has to look under.
        check("stored under the playlist label", on_disk,
              ["Banjo-Kazooie (U) [!].png", "Bomberman 64 (USA).png"])
        check("real bytes", (boxarts / "Bomberman 64 (USA).png").read_bytes(),
              PNG)
        check("no .part left behind",
              list(dest.rglob("*.part")), [])

        print("\nresumability")
        plans2, _ = artwork.plan_all(playlists, dest_root=dest)
        check("second plan wants nothing", len(plans2[0].wanted), 0)
        check("second plan sees them present", plans2[0].present, 2)

        (boxarts / "Bomberman 64 (USA).png").unlink()
        plans3, _ = artwork.plan_all(playlists, dest_root=dest)
        check("only the gap is re-fetched",
              [i.dest.name for i in plans3[0].wanted],
              ["Bomberman 64 (USA).png"])

        print("\npartial failure")
        shutil.rmtree(boxarts)
        Fixture.served = 0
        Fixture.fail_after = 1
        plans4, _ = artwork.plan_all(playlists, dest_root=dest)
        report4 = artwork.run_plan(plans4[0], workers=1, progress=False)
        check("some succeeded", report4.downloaded, 1)
        check("some failed", report4.failed, 1)
        check("failure is explained", bool(report4.reason), True)
        check("partial tree is clean", list(dest.rglob("*.part")), [])
        check("what arrived is usable",
              len(list(boxarts.glob("*.png"))), 1)

        Fixture.fail_after = -1
        plans5, _ = artwork.plan_all(playlists, dest_root=dest)
        report5 = artwork.run_plan(plans5[0], workers=1, progress=False)
        check("retry completes the tree",
              report5.downloaded + plans5[0].present, 2)

        print("\na playlist padmap does not own")
        # `.lpl` files are RetroArch's, and users edit them. Every shape that
        # is not a playlist has to come back empty rather than raise: plan_all
        # walks a whole directory, and one bad file must not cost the other
        # nine. This used to be covered against the exporter's reader, which
        # went with the front-end; artwork.read_playlist_items is the only one
        # left, and it had the defect the old coverage was written for --
        # `{"items": [1, 2, 3]}` reached `item.get("label")` and raised
        # AttributeError out of `padmap fetch-art`.
        hostile = work / "hostile"
        hostile.mkdir()
        shapes = {
            "not json at all": "{ this is not json",
            "json but not an object": "[1, 2, 3]",
            "no items key": '{"version": "1.5"}',
            "items is not a list": '{"items": {"a": 1}}',
            "items of numbers": '{"items": [1, 2, 3]}',
            "items of strings": '{"items": ["a", "b"]}',
            "items of nulls": '{"items": [null, null]}',
            "items of lists": '{"items": [[{"label": "x"}]]}',
            "empty file": "",
            "deeply nested": '{"items": ' + "[" * 20000 + "]" * 20000 + "}",
        }
        for name, text in shapes.items():
            target = hostile / "Damaged.lpl"
            target.write_text(text)
            try:
                items, _core = artwork.read_playlist_items(target)
            except Exception as error:  # noqa: BLE001
                check(f"{name} -> no exception",
                      f"{type(error).__name__}: {error}", "no exception")
                continue
            check(f"{name} -> nothing usable", items, [])

        # ...and one damaged entry costs that entry, not the playlist.
        target = hostile / "Damaged.lpl"
        target.write_text(
            '{"items": [1, {"label": "Keeper", "path": "/r/k.z64"}, null]}')
        items, _core = artwork.read_playlist_items(target)
        check("a good entry beside junk survives",
              [item.get("label") for item in items], ["Keeper"])

        # A binary file, because `errors="replace"` is what stops a non-UTF-8
        # byte ending the walk before the parse is even reached.
        target.write_bytes(bytes(range(256)) * 20)
        check("binary -> nothing usable",
              artwork.read_playlist_items(target)[0], [])

        print("\nunreachable server")
        os.environ[artwork.ENV_SERVER] = "http://127.0.0.1:1"
        _, fatal = artwork.plan_all(playlists, dest_root=dest)
        check("reports rather than raises", bool(fatal), True)
    finally:
        server.shutdown()
        shutil.rmtree(work, ignore_errors=True)
        os.environ.pop(artwork.ENV_SERVER, None)

    print()
    if FAILURES:
        print(f"{len(FAILURES)} failure(s): {', '.join(FAILURES)}")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
