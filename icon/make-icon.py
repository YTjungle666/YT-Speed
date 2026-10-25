#!/usr/bin/env python3
"""speedtest-rs 图标 v2：都走「速度表」这一个隐喻，差别只在可辨识度的做法。

v1 的对比图纸判定：表盘最容易读作测速，但白色细指针在 16px 和白底标签栏里会消失。
所以 v2 三个变体分别解决这件事：
  g1 深色圆盘 + 白色楔形指针（指针做成三角，不做细线）
  g2 深色圆盘 + 弧上滑块（不用指针，弧本身带一个亮块，16px 也立得住）
  g3 无底盘 + 加粗弧 + 楔形指针（最贴页面底色，深色标签栏里最干净）
"""
import argparse
import math
import pathlib

from PIL import Image, ImageDraw

S = 4
RESAMPLE = Image.Resampling.LANCZOS
BG = "#0b0e13"
ACCENT = "#4da3ff"
TILE = "#101724"
WHITE = "#e8edf5"


def canvas(size=512, bg=(0, 0, 0, 0)):
    im = Image.new("RGBA", (size * S, size * S), bg)
    return im, ImageDraw.Draw(im)


def px(*v):
    return tuple(int(x * S) for x in v)


def arc(d, cx, cy, r, w, start, end, color):
    d.arc(px(cx - r, cy - r, cx + r, cy + r), start=start, end=end, fill=color, width=int(w * S))


def poly(d, pts, color):
    d.polygon([(x * S, y * S) for x, y in pts], fill=color)


def dot(d, x, y, r, color):
    d.ellipse(px(x - r, y - r, x + r, y + r), fill=color)


def wedge(d, cx, cy, ang_deg, length, half_w, color):
    a = math.radians(ang_deg)
    tx, ty = cx + math.cos(a) * length, cy + math.sin(a) * length
    nx, ny = -math.sin(a), math.cos(a)
    poly(d, [(tx, ty), (cx + nx * half_w, cy + ny * half_w), (cx - nx * half_w, cy - ny * half_w)], color)


def tile(im, d, color=TILE, inset=0):
    """深色圆盘：白底标签栏里也能立住，圆形裁切天然适配。"""
    d.ellipse(px(inset, inset, 512 - inset, 512 - inset), fill=color)


# g1：圆盘 + 楔形指针
def g1():
    im, d = canvas()
    tile(im, d)
    cx, cy, r = 256, 268, 146
    arc(d, cx, cy, r, 40, 150, 390, ACCENT)
    wedge(d, cx, cy, -46, 118, 30, WHITE)
    dot(d, cx, cy, 34, WHITE)
    dot(d, cx, cy, 15, TILE)
    return im


# g2：圆盘 + 弧上滑块（无指针）
def g2():
    im, d = canvas()
    tile(im, d)
    cx, cy, r = 256, 268, 146
    arc(d, cx, cy, r, 44, 150, 390, ACCENT)
    # 弧上的亮块：放在 60° 处，16px 下就是弧上一点高光
    a = math.radians(-52)
    dot(d, cx + math.cos(a) * r, cy + math.sin(a) * r, 40, WHITE)
    return im


# g3：无底盘，仅弧 + 楔形指针
def g3():
    im, d = canvas()
    cx, cy, r = 256, 262, 150
    arc(d, cx, cy, r, 46, 150, 390, ACCENT)
    wedge(d, cx, cy, -50, 112, 32, WHITE)
    dot(d, cx, cy, 36, WHITE)
    return im


CANDS = {"g1-tile-needle": g1, "g2-tile-marker": g2, "g3-bare-needle": g3}


def resize(im, n):
    return im.convert("RGBA").resize((n, n), RESAMPLE)


def circle_mask(im):
    n = im.size[0]
    m = Image.new("L", (n * 4, n * 4), 0)
    ImageDraw.Draw(m).ellipse((0, 0, n * 4 - 1, n * 4 - 1), fill=255)
    out = im.copy()
    out.putalpha(m.resize((n, n), RESAMPLE))
    return out


def sheet(pngs, out):
    pad = 16
    W = pad + 160 + 150 + 100 + 60 + 60 + 80
    H = pad + len(pngs) * 120
    sk = Image.new("RGBA", (W, H), "#12151a")
    d = ImageDraw.Draw(sk)
    d.text((pad, 2), "160   圆裁128   64   32   16   16(白底)", fill="#8b97a8")
    for i, (name, im) in enumerate(pngs):
        y = pad + 16 + i * 120
        sk.alpha_composite(resize(im, 96), (pad, y + 6))
        x = pad + 150
        sk.alpha_composite(circle_mask(resize(im, 96)), (x, y + 6))
        x += 110
        sk.alpha_composite(resize(im, 64), (x + 8, y + 22))
        x += 90
        sk.alpha_composite(resize(im, 32), (x + 8, y + 38))
        x += 55
        sk.alpha_composite(resize(im, 16), (x + 8, y + 46))
        x += 45
        d.rectangle((x + 4, y + 38, x + 36, y + 70), fill="#ffffff")
        sk.alpha_composite(resize(im, 16), (x + 14, y + 46))
        d.text((pad + 2, y + 104), name, fill="#c9d1d9")
    sk.convert("RGB").save(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="./icon")
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    pngs = []
    for name, fn in CANDS.items():
        im = fn()
        im.resize((512, 512), RESAMPLE).save(out / f"{name}.png")
        pngs.append((name, im))
        print(out / f"{name}.png")
    sheet(pngs, out / "sheet.png")
    print(out / "sheet.png")


if __name__ == "__main__":
    main()
