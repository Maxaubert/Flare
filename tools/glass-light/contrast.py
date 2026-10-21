"""Task 57, step 2: WCAG contrast of ANSI 1-6 and 9-14 against the effective ground of a see-through
light theme, `mix(desktop, ground, alpha)`, alpha = 1 - level/100, for desktops black, 128 grey and white
and levels 30, 50 and 70. Reads palettes.csv (a golden kept by crates/flare-theme/tests/glass_light.rs),
writes contrast.csv beside it and prints the worst ten rows.

    python tools/glass-light/contrast.py [--out DIR]
"""
import argparse
import csv
import os

HERE = os.path.dirname(os.path.abspath(__file__))
DESKTOPS = {"black": (0, 0, 0), "grey128": (128, 128, 128), "white": (255, 255, 255)}
LEVELS = (30, 50, 70)
ANSI = (1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14)
NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]


def parse_hex(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


def luminance(rgb):
    def lin(c):
        c /= 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (lin(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def effective_ground(desktop, ground, alpha):
    """What the eye sees behind the text: the ground over the desktop at `alpha`."""
    return tuple(round(d + (g - d) * alpha) for d, g in zip(desktop, ground))


def ansi_name(i):
    return ("bright " if i >= 8 else "") + NAMES[i % 8]


def rows(palettes):
    out = []
    for p in palettes:
        ground = parse_hex(p["bg"])
        for level in LEVELS:
            alpha = 1.0 - level / 100.0
            for dname, desk in DESKTOPS.items():
                eff = effective_ground(desk, ground, alpha)
                for i in ANSI:
                    c = p.get(f"a{i}", "")
                    if not c:
                        continue
                    out.append({
                        "theme": p["id"], "level": level, "desktop": dname, "ansi": i,
                        "name": ansi_name(i), "colour": c,
                        "ground": "#%02x%02x%02x" % eff,
                        "ratio": round(contrast(parse_hex(c), eff), 2),
                    })
    return out


def worst(table, n=10):
    return sorted(table, key=lambda r: r["ratio"])[:n]


def load(path):
    with open(path, newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--palettes", default=os.path.join(HERE, "palettes.csv"))
    ap.add_argument("--out", default=HERE)
    a = ap.parse_args()
    table = rows(load(a.palettes))
    os.makedirs(a.out, exist_ok=True)
    path = os.path.join(a.out, "contrast.csv")
    with open(path, "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=list(table[0]))
        w.writeheader()
        w.writerows(table)
    print(f"{len(table)} rows -> {path}")
    for r in worst(table):
        print(f"{r['ratio']:5.2f}  {r['theme']:<8} L{r['level']} {r['desktop']:<7} {r['name']:<15} {r['colour']} on {r['ground']}")


if __name__ == "__main__":
    main()
