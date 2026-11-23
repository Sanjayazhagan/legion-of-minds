#!/usr/bin/env python3
"""Make site/terms.html and site/privacy.html from the Markdown in legal/.

Run after editing either document:  python3 legal/build_site_pages.py
The game shows the same Markdown (game/crates/app/src/legal.rs), so the
launch page, the store page links and the game always say the same thing.
"""
import html
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} - Legion of Minds</title>
<meta name="theme-color" content="#0b0d14">
<link rel="icon" href="media/icon.png">
<style>
:root {{ --night: #0b0d14; --panel: #161a26; --line: #262c3d; --gold: #f6b72e; --text: #eef1f7; --muted: #9ba4b8; color-scheme: dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; background: var(--night); color: var(--text); font: 17px/1.65 "Segoe UI", system-ui, sans-serif; }}
main {{ max-width: 760px; margin: 0 auto; padding: 40px 16px 80px; }}
a {{ color: var(--gold); }}
nav {{ display: flex; gap: 18px; flex-wrap: wrap; font-size: 15px; margin-bottom: 28px; }}
h1 {{ color: var(--gold); font-size: 34px; line-height: 1.2; margin: 0 0 6px; }}
h2 {{ font-size: 22px; margin: 34px 0 6px; }}
ul {{ padding-left: 22px; }}
li {{ margin: 4px 0; }}
p.version {{ color: var(--muted); font-size: 15px; }}
</style>
</head>
<body>
<main>
<nav><a href="index.html">Legion of Minds</a><a href="terms.html">Terms of Service</a><a href="privacy.html">Privacy Policy</a></nav>
{body}
</main>
</body>
</html>
"""


def inline(s: str) -> str:
    s = html.escape(s, quote=False)
    s = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", s)
    s = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', s)
    return s


def convert(md: str) -> tuple[str, str]:
    out, para, items, title = [], [], [], ""

    def flush():
        if para:
            text = " ".join(para)
            cls = ' class="version"' if text.startswith("Version ") else ""
            out.append(f"<p{cls}>{inline(text)}</p>")
            para.clear()
        if items:
            out.append("<ul>" + "".join(f"<li>{inline(i)}</li>" for i in items) + "</ul>")
            items.clear()

    for line in md.splitlines():
        line = line.rstrip()
        if line.startswith("# "):
            flush()
            title = line[2:]
            out.append(f"<h1>{inline(title)}</h1>")
        elif line.startswith("## "):
            flush()
            out.append(f"<h2>{inline(line[3:])}</h2>")
        elif line.startswith("- "):
            if para:
                flush()
            items.append(line[2:])
        elif not line.strip():
            flush()
        elif items:
            items[-1] += " " + line.strip()
        else:
            para.append(line.strip())
    flush()
    return title, "\n".join(out)


for src, dst in [("TERMS_OF_SERVICE.md", "terms.html"), ("PRIVACY_POLICY.md", "privacy.html")]:
    title, body = convert((ROOT / "legal" / src).read_text(encoding="utf-8"))
    (ROOT / "site" / dst).write_text(PAGE.format(title=title, body=body), encoding="utf-8")
    print("wrote site/" + dst)
