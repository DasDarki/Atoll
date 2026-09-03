#!/usr/bin/env python3
"""Keeps the translation files in step with the strings in the source.

The strings of Atoll are German and live in the source: `t!("...")` in Rust and `@tr("...")` in
Slint. Every other language is a gettext file under `lang/<language>/LC_MESSAGES/atoll.po`, which
both Slint and `atoll-i18n` read.

    scripts/translations.py report     what is missing, what is no longer used
    scripts/translations.py update     adds new strings, drops what is gone, sorts the file
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANGUAGES = ["en"]

RUST_CALL = re.compile(r'(?<![\w!])t!\(\s*("(?:[^"\\]|\\.)*")')
SLINT_CALL = re.compile(
    r'@tr\(\s*("(?:[^"\\]|\\.)*")(\s*\|\s*("(?:[^"\\]|\\.)*"))?'
)
PO_ENTRY = re.compile(
    r'^msgid (?P<id>"(?:[^"\\]|\\.)*")\n'
    r'(?:msgid_plural (?P<many>"(?:[^"\\]|\\.)*")\n)?'
    r'(?:msgstr (?P<text>"(?:[^"\\]|\\.)*")\n'
    r'|msgstr\[0\] (?P<one_text>"(?:[^"\\]|\\.)*")\n'
    r'msgstr\[1\] (?P<many_text>"(?:[^"\\]|\\.)*")\n)',
    re.M,
)

HEADER = """# Atoll in {language}. The strings in the source are German, this file translates them.
# Both the user interface, which Slint bundles, and the Rust side read this file.
# Run scripts/translations.py update after changing a string in the source.
msgid ""
msgstr ""
"Project-Id-Version: Atoll\\n"
"Language: {language}\\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\\n"
"""


def unquote(literal):
    """The text of a `"..."` of Rust, Slint or gettext."""
    result = []
    escape = False
    for character in literal[1:-1]:
        if escape:
            result.append({"n": "\n", "t": "\t", "r": "\r"}.get(character, character))
            escape = False
        elif character == "\\":
            escape = True
        else:
            result.append(character)
    return "".join(result)


def quote(text):
    escaped = (
        text.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\t", "\\t")
    )
    return f'"{escaped}"'


def strings_in_source():
    """Every translatable string, and which of them are the plural of another."""
    found = []
    plural = {}
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "/tests/" in str(path):
            continue
        for match in RUST_CALL.finditer(path.read_text()):
            found.append(unquote(match.group(1)))
    for path in sorted((ROOT / "crates/atoll/ui").rglob("*.slint")):
        for match in SLINT_CALL.finditer(path.read_text()):
            one = unquote(match.group(1))
            found.append(one)
            if match.group(3):
                plural[one] = unquote(match.group(3))
    return dict.fromkeys(found), plural


def read_catalog(path):
    """What is already translated, by the German original."""
    if not path.is_file():
        return {}
    text = path.read_text()
    entries = {}
    for match in PO_ENTRY.finditer(text):
        original = unquote(match.group("id"))
        if not original:
            continue
        if match.group("text") is not None:
            entries[original] = unquote(match.group("text"))
        else:
            entries[original] = unquote(match.group("one_text"))
            entries[unquote(match.group("many"))] = unquote(match.group("many_text"))
    return entries


def write_catalog(path, language, wanted, plural, translations):
    lines = [HEADER.format(language=language)]
    written = set()
    for text in sorted(wanted):
        if text in written:
            continue
        written.add(text)
        if text in plural:
            many = plural[text]
            written.add(many)
            lines.append(
                f"msgid {quote(text)}\n"
                f"msgid_plural {quote(many)}\n"
                f"msgstr[0] {quote(translations.get(text, ''))}\n"
                f"msgstr[1] {quote(translations.get(many, ''))}\n"
            )
        elif text in plural.values():
            continue
        else:
            lines.append(f"msgid {quote(text)}\nmsgstr {quote(translations.get(text, ''))}\n")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines))


def main():
    action = sys.argv[1] if len(sys.argv) > 1 else "report"
    wanted, plural = strings_in_source()
    problems = 0
    for language in LANGUAGES:
        path = ROOT / "lang" / language / "LC_MESSAGES" / "atoll.po"
        translations = read_catalog(path)
        needed = list(wanted) + [many for many in plural.values()]
        missing = [text for text in needed if not translations.get(text)]
        unused = [text for text in translations if text not in needed]
        print(f"{language}: {len(wanted)} strings, {len(missing)} untranslated, {len(unused)} no longer used")
        for text in missing:
            print(f"  untranslated: {text!r}")
        for text in unused:
            print(f"  no longer used: {text!r}")
        problems += len(missing)
        if action == "update":
            write_catalog(path, language, wanted, plural, translations)
            print(f"  wrote {path.relative_to(ROOT)}")
    return 1 if problems and action == "report" else 0


if __name__ == "__main__":
    sys.exit(main())
