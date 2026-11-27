#!/usr/bin/env python3
"""Generate an isolated Regent Launcher dev/screenshot fixture tree.

Matches `game::Roots::under(dir)` layout. Writes ONLY inside the target dir.
"""
import json
import os
import shutil
import struct
import sys
import tempfile
import time
import zipfile
import zlib
from pathlib import Path

SENTINEL = ".regent-fixture"


def make_png(width: int, height: int, rgb_fn) -> bytes:
    """Create a valid RGB PNG using stdlib zlib only (no external deps)."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)  # filter type 0
        for x in range(width):
            r, g, b = rgb_fn(x, y, width, height)
            raw.extend((r & 255, g & 255, b & 255))

    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
        + chunk(b"IEND", b"")
    )


def solid_badge_png(r: int, g: int, b: int, size: int = 48) -> bytes:
    def px(x, y, w, h):
        border = x < 2 or y < 2 or x >= w - 2 or y >= h - 2
        if border:
            return (min(255, r + 40), min(255, g + 40), min(255, b + 40))
        diag = abs(x - y) < 4 or abs((w - 1 - x) - y) < 4
        if diag:
            return (min(255, r + 24), min(255, g + 24), min(255, b + 24))
        return (r, g, b)

    return make_png(size, size, px)


def backdrop_png() -> bytes:
    """Subtle dark maritime/starfield backdrop for Stellaris Home."""
    def px(x, y, w, h):
        # Soft deep navy to slate radial falloff on right
        dx = (x - w * 0.72) / (w * 0.55)
        dy = (y - h * 0.44) / (h * 0.6)
        d = max(0.0, 1.0 - (dx * dx + dy * dy))
        r = int(11 + d * 28)
        g = int(14 + d * 36)
        b = int(20 + d * 56)
        # Deterministic tiny stars
        hval = ((x * 73856093) ^ (y * 19349663)) & 0xFFFF
        if hval < 18:
            return (215, 200, 168)
        if hval < 40:
            return (111, 155, 216)
        return (r, g, b)

    return make_png(320, 180, px)


def logo_png() -> bytes:
    """Compact brass-accented placeholder game logo."""
    def px(x, y, w, h):
        if y in (0, 1, h - 2, h - 1) or x in (0, 1, w - 2, w - 1):
            return (201, 164, 92)
        cx, cy = w // 2, h // 2
        if abs(y - cy) <= 2 and 16 <= x <= w - 16:
            return (201, 164, 92)
        if abs(x - cx) <= 2 and 10 <= y <= h - 10:
            return (232, 228, 217)
        return (17, 21, 29)

    return make_png(220, 56, px)


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def write_bytes(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def write_sav(path: Path, name: str, date: str, version: str, mtime: float) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    meta = f'version="{version}"\nname="{name}"\ndate="{date}"\n'
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as zf:
        zf.writestr("meta", meta)
        zf.writestr("gamestate", "name=\"" + name + "\"\n")
    os.utime(path, (mtime, mtime))


def build_stellaris(base: Path) -> None:
    gdir = base / "steam/steamapps/common/Stellaris"
    wsdir = base / "steam/steamapps/workshop/content/281990"
    acf_path = base / "steam/steamapps/workshop/appworkshop_281990.acf"
    ddir = base / "docs/Paradox Interactive/Stellaris"

    write_text(
        gdir / "launcher-settings.json",
        json.dumps(
            {
                "gameId": "stellaris",
                "version": "Corona Borealis v4.6.0 (3d1a)",
                "rawVersion": "v4.6.0",
                "gameDataPath": "~/Documents/Paradox Interactive/Stellaris",
                "exePath": "/usr/bin/true",
                "exeArgs": ["-gdpr-compliant"],
            },
            indent=2,
        ),
    )
    write_text(
        gdir / "checksum_manifest.txt",
        "directory\nname = common\nsub_directories = yes\nfile_extension = .txt\n\n"
        "directory\nname = events\nsub_directories = yes\n",
    )
    write_text(
        gdir / "settings-layout.json",
        json.dumps(
            [
                {
                    "name": "System",
                    "settings": [
                        {
                            "name": "language",
                            "options": [
                                {"label": "English", "value": "l_english"},
                                {"label": "简体中文", "value": "l_simp_chinese"},
                                {"label": "Deutsch", "value": "l_german"},
                                {"label": "Français", "value": "l_french"},
                                {"label": "日本語", "value": "l_japanese"},
                            ],
                        }
                    ],
                }
            ],
            indent=2,
            ensure_ascii=False,
        ),
    )
    write_bytes(gdir / "assets/app-background.png", backdrop_png())
    write_bytes(gdir / "assets/game-logo.png", logo_png())

    # 36 categorized DLCs: 14 expansion, 6 story_pack, 7 species_pack, 9 content_pack
    dlc_specs = [
        ("expansion", "Utopia"),
        ("expansion", "Apocalypse"),
        ("expansion", "MegaCorp"),
        ("expansion", "Federations"),
        ("expansion", "Nemesis"),
        ("expansion", "Overlord"),
        ("expansion", "Galactic Paragons"),
        ("expansion", "The Machine Age"),
        ("expansion", "Cosmic Storms"),
        ("expansion", "Biogenesis"),
        ("expansion", "Shadows of the Shroud"),
        ("expansion", "Astral Planes"),
        ("expansion", "First Contact"),
        ("expansion", "Grand Archive"),
        ("story_pack", "Leviathans Story Pack"),
        ("story_pack", "Synthetic Dawn Story Pack"),
        ("story_pack", "Distant Stars Story Pack"),
        ("story_pack", "Ancient Relics Story Pack"),
        ("story_pack", "Horizon Signal"),
        ("story_pack", "Rick the Cube"),
        ("species_pack", "Plantoids Species Pack"),
        ("species_pack", "Humanoids Species Pack"),
        ("species_pack", "Lithoids Species Pack"),
        ("species_pack", "Necroids Species Pack"),
        ("species_pack", "Aquatics Species Pack"),
        ("species_pack", "Toxoids Species Pack"),
        ("species_pack", "Infernals Species Pack"),
        ("content_pack", "Creatures of the Void"),
        ("content_pack", "Anniversary Portraits"),
        ("content_pack", "Arachnoid Portrait Pack"),
        ("content_pack", "Symbols of Domination"),
        ("content_pack", "Digital OST"),
        ("content_pack", "Ebook: Infinite Frontiers"),
        ("content_pack", "Sign-up Campaign Bonus"),
        ("content_pack", "Ringtones Pack"),
        ("content_pack", "Nova Edition Upgrade"),
    ]
    dlc_thumb_bytes = solid_badge_png(56, 86, 118, 44)
    for idx, (cat, name) in enumerate(dlc_specs, start=1):
        slug = f"dlc{idx:03d}"
        folder = gdir / "dlc" / f"{slug}_{cat}"
        affects = "yes" if cat == "expansion" and idx <= 2 else "no"
        thumb_line = 'thumbnail = "thumbnail.png"\n' if idx <= 8 else ""
        write_text(
            folder / f"{slug}.dlc",
            f'name = "{name}"\n'
            f'archive = "dlc/{slug}_{cat}/{slug}.zip"\n'
            f'category = "{cat}"\n'
            f"affects_checksum = {affects}\n"
            f"{thumb_line}",
        )
        write_bytes(folder / f"{slug}.zip", b"PK\x05\x06" + b"\x00" * 18)
        if idx <= 8:
            write_bytes(folder / "thumbnail.png", dlc_thumb_bytes)

    # 1000 mods: 985 workshop + 15 local
    # First 8 workshop mods are rich headline mods with thumbs, conflicts, deps, mismatches, CJK
    now = int(time.time())
    headlines = [
        (
            "1623423360",
            "UI Overhaul Dynamic",
            "4.6.2",
            "v4.6.*",
            [],
            236_163_907,
            now - 86_400 * 2,
            (46, 98, 134),
            [("gfx/interface/hud.dds", "hud"), ("common/defines/ui_defines.txt", "a=1\n")],
        ),
        (
            "1628912584",
            "UI Overhaul Dynamic - Tiny Outliner",
            "4.6.1",
            "v4.6.*",
            ["UI Overhaul Dynamic"],
            4_812_000,
            now - 86_400 * 3,
            (64, 114, 146),
            [("common/defines/ui_defines.txt", "a=2\n"), ("gfx/interface/outliner.dds", "o")],
        ),
        (
            "1121692237",
            "Gigastructural Engineering & More (4.6)",
            "4.6.0",
            "v4.4.*",
            [],
            482_900_120,
            now - 86_400 * 14,
            (156, 94, 48),
            [
                ("common/buildings/giga_buildings.txt", "b=1\n"),
                ("common/defines/ui_defines.txt", "a=3\n"),
                ("events/giga_crisis.txt", "e=1\n"),
            ],
        ),
        (
            "3090328185",
            "Planetary Shields",
            "4.4.3",
            "v4.4.*",
            [],
            12_093_428,
            now - 86_400 * 31,
            (88, 136, 110),
            [("common/buildings/giga_buildings.txt", "b=2\n")],
        ),
        (
            "3339549399",
            "星海汉化增强包 · Chinese Localization+",
            "4.6.0",
            "v4.6.*",
            [],
            18_638_280,
            now - 86_400 * 5,
            (164, 72, 66),
            [("localisation/simp_chinese/l_extra.yml", "l_simp_chinese:\n")],
        ),
        (
            "2044599287",
            "Real Space 3.9",
            "3.9.4",
            "v4.5.*",
            [],
            128_440_000,
            now - 86_400 * 45,
            (72, 64, 132),
            [("gfx/models/stars/star.dds", "s")],
        ),
    ]

    acf_lines = [
        '"AppWorkshop"',
        "{",
        '\t"appid"\t\t"281990"',
        '\t"WorkshopItemsInstalled"',
        "\t{",
    ]

    for wid, name, ver, sup, deps, size, updated, rgb, files in headlines:
        mdir = wsdir / wid
        dep_block = ""
        if deps:
            items = "\n".join(f'\t"{d}"' for d in deps)
            dep_block = f"dependencies={{\n{items}\n}}\n"
        write_text(
            mdir / "descriptor.mod",
            f'name="{name}"\n'
            f'version="{ver}"\n'
            f'supported_version="{sup}"\n'
            f'picture="thumbnail.png"\n'
            f'remote_file_id="{wid}"\n'
            f"{dep_block}",
        )
        write_bytes(mdir / "thumbnail.png", solid_badge_png(*rgb))
        for rel, body in files:
            write_text(mdir / rel, body)
        acf_lines.extend(
            [
                f'\t\t"{wid}"',
                "\t\t{",
                f'\t\t\t"size"\t\t"{size}"',
                f'\t\t\t"timeupdated"\t\t"{updated}"',
                "\t\t}",
            ]
        )

    # Bulk workshop mods to reach 985 workshop mods
    wsdir.mkdir(parents=True, exist_ok=True)
    total_ws = 985
    bulk_count = total_ws - len(headlines)
    for i in range(1, bulk_count + 1):
        wid = str(2_500_000_000 + i)
        mdir = wsdir / wid
        mdir.mkdir(exist_ok=True)
        sup = "v4.4.*" if i % 47 == 0 else "v4.6.*"
        (mdir / "descriptor.mod").write_text(
            f'name="Stellaris Compendium Module {i:04d}"\n'
            f'version="1.{i % 9}.0"\n'
            f'supported_version="{sup}"\n'
            f'remote_file_id="{wid}"\n',
            encoding="utf-8",
        )
        size = 512_000 + (i * 173_117) % 95_000_000
        updated = now - (i * 43_200) % (86_400 * 365)
        acf_lines.extend(
            [
                f'\t\t"{wid}"',
                "\t\t{",
                f'\t\t\t"size"\t\t"{size}"',
                f'\t\t\t"timeupdated"\t\t"{updated}"',
                "\t\t}",
            ]
        )

    acf_lines.extend(["\t}", "}"])
    write_text(acf_path, "\n".join(acf_lines) + "\n")

    # 15 local mods (total = 1000 mods)
    local_mod_dir = ddir / "mod"
    local_mod_dir.mkdir(parents=True, exist_ok=True)
    for i in range(1, 16):
        slug = f"local_pack_{i:02d}"
        target = local_mod_dir / slug
        write_text(target / "gfx/flag.dds", f"local-{i}")
        write_text(
            local_mod_dir / f"{slug}.mod",
            f'name="Local Custom Patch {i:02d}"\n'
            f'version="0.{i}.0"\n'
            f'supported_version="v4.6.*"\n'
            f'path="mod/{slug}"\n',
        )

    # Settings (CRLF)
    game_settings = (
        'language="l_english"\r\n'
        "graphics=\r\n{\r\n"
        "\tsize=\r\n\t{\r\n\t\tx=2560\r\n\t\ty=1440\r\n\t}\r\n"
        "\tgui_scale=1.000000\r\n"
        "\tfullScreen=no\r\n"
        "\tborderless=yes\r\n"
        "\tvsync=yes\r\n"
        "}\r\n"
        "master_volume=80.000000\r\n"
        "music_volume=65.000000\r\n"
        "sound_fx_volume=75.000000\r\n"
        "ambient_volume=60.000000\r\n"
        "voice_volume=85.000000\r\n"
        "autosave=4\r\n"
    )
    pdx_settings = (
        '"Graphics"=\r\n{\r\n'
        '\t"display_mode"=\r\n\t{\r\n\t\tvalue="borderless_fullscreen"\r\n\t\tversion=0\r\n\t}\r\n'
        '\t"vsync"=\r\n\t{\r\n\t\tenabled=yes\r\n\t\tversion=0\r\n\t}\r\n'
        '\t"fullscreen_resolution"=\r\n\t{\r\n\t\tvalue="2560x1440"\r\n\t\tversion=0\r\n\t}\r\n'
        "}\r\n"
        '"System"=\r\n{\r\n'
        '\t"language"=\r\n\t{\r\n\t\tvalue="l_english"\r\n\t\tversion=0\r\n\t}\r\n'
        "}\r\n"
    )
    write_bytes(ddir / "settings.txt", game_settings.encode("utf-8"))
    write_bytes(ddir / "pdx_settings.txt", pdx_settings.encode("utf-8"))

    # Saves (3 zip saves)
    saves_dir = ddir / "save games"
    write_sav(
        saves_dir / "united_nations_of_earth/2245.03.01.sav",
        "United Nations of Earth",
        "2245.03.01",
        "Corona Borealis v4.6.0",
        now - 3600,
    )
    write_sav(
        saves_dir / "commonwealth_of_man/2312.08.15.sav",
        "Commonwealth of Man",
        "2312.08.15",
        "Corona Borealis v4.6.0",
        now - 86_400 * 2,
    )
    write_sav(
        saves_dir / "blorg_commonality/2201.01.01.sav",
        "Blorg Commonality",
        "2201.01.01",
        "Circinus v4.4.0",
        now - 86_400 * 10,
    )

    # Crash dirs (2) + live log
    c1 = ddir / "crashes/stellaris_20261125_140210"
    write_text(
        c1 / "meta.yml",
        "AppVersion: Corona Borealis v4.6.0\n"
        "ChecksumStatus: Modified\n"
        "Mod_1: UI Overhaul Dynamic\n"
        "Mod_2: Gigastructural Engineering & More (4.6)\n"
        "Mod_3: Planetary Shields\n",
    )
    write_text(c1 / "exception.txt", "Application: Stellaris\nCaught signal 11 (SIGSEGV)\n")
    write_text(
        c1 / "error.log",
        "[14:01:55][effect.cpp:211]: Invalid scope at events/giga_crisis.txt:142\n"
        "[14:02:04][trigger.cpp:89]: Unknown modifier in events/giga_crisis.txt:218\n"
        "[14:02:06][building.cpp:44]: Duplicate key in common/buildings/giga_buildings.txt:12\n"
        "[14:02:09][effect.cpp:502]: Fatal trigger failure in events/giga_crisis.txt:301\n",
    )

    c2 = ddir / "crashes/stellaris_20261120_091500"
    write_text(
        c2 / "meta.yml",
        "AppVersion: Corona Borealis v4.6.0\n"
        "ChecksumStatus: Modified\n"
        "Mod_1: UI Overhaul Dynamic\n"
        "Mod_2: Uninstalled Legacy Overhaul\n",
    )
    write_text(
        c2 / "error.log",
        "[09:14:58][gui.cpp:72]: Missing texture gfx/interface/hud.dds\n"
        "[09:15:00][script.cpp:19]: Missing scripted effect common/ghost/missing.txt:8\n",
    )

    write_text(
        ddir / "logs/error.log",
        "[18:30:01][gui.cpp:72]: Minor warning in gfx/interface/hud.dds\n",
    )

    # Seed playsets.json with intentional dep order issue (Tiny Outliner before UI Overhaul Dynamic)
    # and conflicts/mismatches enabled so every page has live data immediately.
    playsets = {
        "games": {
            "stellaris": {
                "active": 0,
                "launch_args": "",
                "playsets": [
                    {
                        "name": "Galaxy Campaign",
                        "mods": [
                            {"id": "mod/ugc_1628912584.mod", "enabled": True},
                            {"id": "mod/ugc_1623423360.mod", "enabled": True},
                            {"id": "mod/ugc_1121692237.mod", "enabled": True},
                            {"id": "mod/ugc_3090328185.mod", "enabled": True},
                            {"id": "mod/ugc_3339549399.mod", "enabled": True},
                            {"id": "mod/ugc_2044599287.mod", "enabled": False},
                        ],
                        "disabled_dlcs": ["dlc/dlc009_expansion/dlc009.dlc"],
                    },
                    {
                        "name": "Ironman Clean UI",
                        "mods": [
                            {"id": "mod/ugc_3339549399.mod", "enabled": True},
                        ],
                        "disabled_dlcs": [],
                    },
                ],
            }
        }
    }
    write_text(
        base / "data/regent-launcher/playsets.json",
        json.dumps(playsets, indent=2),
    )


def build_eu4(base: Path) -> None:
    """Second game WITHOUT assets/app-background.png or game-logo.png to test star-chart fallback."""
    gdir = base / "steam/steamapps/common/Europa Universalis IV"
    ddir = base / "docs/Paradox Interactive/Europa Universalis IV"

    write_text(
        gdir / "launcher-settings.json",
        json.dumps(
            {
                "gameId": "eu4",
                "version": "EU4 v1.37.5.0 Inca (491d)",
                "rawVersion": "v1.37.5",
                "gameDataPath": "~/Documents/Paradox Interactive/Europa Universalis IV",
                "exePath": "/usr/bin/true",
                "exeArgs": [],
            },
            indent=2,
        ),
    )
    write_text(
        gdir / "checksum_manifest.txt",
        "directory\nname = common\nsub_directories = yes\nfile_extension = .txt\n",
    )
    write_text(
        gdir / "dlc/dlc010_art_of_war/dlc010.dlc",
        'name = "Art of War"\narchive = "dlc/dlc010_art_of_war/dlc010.zip"\ncategory = "expansion"\n',
    )
    write_bytes(gdir / "dlc/dlc010_art_of_war/dlc010.zip", b"PK\x05\x06" + b"\x00" * 18)

    write_text(ddir / "mod/fast_universalis/gfx/map.dds", "m")
    write_text(
        ddir / "mod/fast_universalis.mod",
        'name="Fast Universalis"\nversion="1.37"\nsupported_version="v1.37.*"\npath="mod/fast_universalis"\n',
    )
    write_text(
        ddir / "settings.txt",
        'language="l_english"\ngraphics=\n{\n\tsize=\n\t{\n\t\tx=1920\n\t\ty=1080\n\t}\n\tfullScreen=no\n\tborderless=yes\n\tvsync=yes\n}\nmaster_volume=90.000000\n',
    )


def validate_target(raw_arg: str, repo_root: Path) -> Path:
    raw = Path(raw_arg)
    if os.path.islink(raw):
        raise SystemExit(f"refusing symlinked fixture target: {raw_arg}")
    abs_target = Path(os.path.abspath(raw_arg))
    if os.path.islink(abs_target):
        raise SystemExit(f"refusing symlinked fixture target: {raw_arg}")

    # Resolve the nearest existing ancestor to check containment inside repo_root.
    probe = abs_target
    tail_parts = []
    while not probe.exists() and not os.path.islink(probe):
        if probe == probe.parent:
            break
        tail_parts.append(probe.name)
        probe = probe.parent
    if os.path.islink(probe):
        raise SystemExit(f"refusing symlinked ancestor in fixture path: {probe}")
    resolved_ancestor = probe.resolve()
    target = resolved_ancestor
    for part in reversed(tail_parts):
        target = target / part

    try:
        rel = target.relative_to(repo_root)
    except ValueError:
        raise SystemExit(f"refusing fixture target outside repo ({repo_root}): {target}")
    if rel == Path(".") or not rel.parts:
        raise SystemExit(f"refusing to write fixture directly into repo root: {repo_root}")

    # Reject any symlink component between repo_root and target.
    cur = repo_root
    for part in rel.parts:
        cur = cur / part
        if os.path.islink(cur):
            raise SystemExit(f"refusing symlinked path component: {cur}")

    if os.path.lexists(target):
        if os.path.islink(target) or not target.is_dir():
            raise SystemExit(f"refusing non-directory fixture target: {target}")
        sentinel = target / SENTINEL
        if os.path.islink(sentinel) or not sentinel.is_file():
            raise SystemExit(
                f"refusing to delete unmarked directory (missing {SENTINEL} sentinel): {target}"
            )

    return target


def main() -> None:
    if len(sys.argv) < 2:
        print("usage: make_fixture.py <dir>", file=sys.stderr)
        sys.exit(2)
    repo_root = Path(__file__).resolve().parent.parent
    target = validate_target(sys.argv[1], repo_root)
    target.parent.mkdir(parents=True, exist_ok=True)

    staging = Path(tempfile.mkdtemp(prefix=f".{target.name}.staging-", dir=str(target.parent)))
    try:
        write_text(staging / SENTINEL, "regent-fixture\n")
        build_stellaris(staging)
        build_eu4(staging)

        if os.path.lexists(target):
            # Re-verify before swap/delete.
            sentinel = target / SENTINEL
            if os.path.islink(target) or os.path.islink(sentinel) or not sentinel.is_file():
                raise SystemExit(f"target changed unsafely before swap: {target}")
            backup = Path(
                tempfile.mkdtemp(prefix=f".{target.name}.old-", dir=str(target.parent))
            )
            backup.rmdir()
            os.rename(target, backup)
            try:
                os.rename(staging, target)
            except Exception:
                os.rename(backup, target)
                raise
            shutil.rmtree(backup)
        else:
            os.rename(staging, target)
    except Exception:
        if staging.exists():
            shutil.rmtree(staging, ignore_errors=True)
        raise


if __name__ == "__main__":
    main()
