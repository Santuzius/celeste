#!/usr/bin/env python3
"""Format SVG files in place like assets/syncative-icon.svg: one element per line, indented by 4 spaces per level; an element that fits into 80 characters stays on one line, otherwise every attribute gets its own line.

Usage: scripts/format-svg.py FILE.svg [...]
"""
import sys
import xml.etree.ElementTree as ET

SVG_NS = "http://www.w3.org/2000/svg"
WIDTH = 80
INDENT = "    "


def name(tag: str) -> str:
    return tag.split("}", 1)[1] if tag.startswith("{") else tag


def escape(value: str) -> str:
    return value.replace("&", "&amp;").replace("<", "&lt;").replace('"', "&quot;")


def attributes(element: ET.Element, root: bool) -> list[str]:
    attrs = [f'{name(k)}="{escape(v)}"' for k, v in element.attrib.items()]
    return [f'xmlns="{SVG_NS}"'] + attrs if root else attrs


def write(element: ET.Element, depth: int, out: list[str], root: bool = False) -> None:
    pad = INDENT * depth
    tag = name(element.tag)
    attrs = attributes(element, root)
    children = list(element)
    if (element.text or "").strip():
        raise SystemExit(f"text content in <{tag}> is not supported")
    end = ">" if children else " />"
    one_line = pad + "<" + " ".join([tag] + attrs) + end
    if len(one_line) <= WIDTH:
        out.append(one_line)
    else:
        out.append(f"{pad}<{tag}")
        out.extend(f"{pad}{INDENT}{a}" for a in attrs)
        out.append(pad + end.strip())
    if children:
        for child in children:
            write(child, depth + 1, out)
        out.append(f"{pad}</{tag}>")


def main() -> None:
    ET.register_namespace("", SVG_NS)
    for path in sys.argv[1:]:
        root = ET.parse(path).getroot()
        out: list[str] = []
        write(root, 0, out, root=True)
        with open(path, "w") as f:
            f.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
