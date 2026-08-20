// 生成 SessionHub 应用图标（无第三方依赖，纯 Node + zlib 手写 PNG 编码）
// 设计：macOS 圆角矩形底（靛蓝→紫罗兰渐变 + 顶部高光），
// 两张叠放的圆角卡片象征“会话汇聚”，SDF 解析抗锯齿。
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";

const SIZE = 1024;

// ---------- PNG 编码 ----------
const crcTable = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

// ---------- 图形工具 ----------
const lerp = (a, b, t) => a + (b - a) * t;

// 圆角盒 SDF：负值在内部
function sdRoundBox(x, y, cx, cy, hw, hh, r) {
  const qx = Math.abs(x - cx) - (hw - r);
  const qy = Math.abs(y - cy) - (hh - r);
  const ax = Math.max(qx, 0);
  const ay = Math.max(qy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(qx, qy), 0) - r;
}
// SDF → 覆盖率（feather 为过渡带宽，越大越柔）
const cover = (sd, feather) => Math.min(Math.max(0.5 - sd / feather, 0), 1);

// alpha-over（直通 alpha）
function over(dst, sr, sg, sb, sa) {
  if (sa <= 0) return dst;
  const outA = sa + dst[3] * (1 - sa);
  if (outA <= 0) return [0, 0, 0, 0];
  return [
    (sr * sa + dst[0] * dst[3] * (1 - sa)) / outA,
    (sg * sa + dst[1] * dst[3] * (1 - sa)) / outA,
    (sb * sa + dst[2] * dst[3] * (1 - sa)) / outA,
    outA,
  ];
}

// ---------- 颜色 ----------
const BG1 = [125, 141, 252]; // 亮靛蓝
const BG2 = [88, 28, 176]; // 深紫罗兰
const WHITE = [255, 255, 255];
const LINE = [99, 102, 241]; // indigo-500
const SHADOW = [35, 28, 90];

// ---------- 逐像素绘制 ----------
const S = SIZE;
const raw = Buffer.alloc(S * (S * 4 + 1));
let o = 0;
for (let y = 0; y < S; y++) {
  raw[o++] = 0;
  for (let x = 0; x < S; x++) {
    let px = [0, 0, 0, 0];

    // 底：全幅圆角矩形（macOS squircle 近似），对角渐变 + 顶部高光
    const sdBg = sdRoundBox(x, y, S / 2, S / 2, S / 2, S / 2, S * 0.223);
    const bgA = cover(sdBg, 1.6);
    if (bgA > 0) {
      const t = (x + y) / (2 * S);
      let r = lerp(BG1[0], BG2[0], t);
      let g = lerp(BG1[1], BG2[1], t);
      let b = lerp(BG1[2], BG2[2], t);
      const hl = Math.max(0, 1 - y / (S * 0.5)) * 0.16;
      r += (255 - r) * hl;
      g += (255 - g) * hl;
      b += (255 - b) * hl;
      px = over(px, r, g, b, bgA);
    }

    // 后卡（半透明白）
    const sdBack = sdRoundBox(x, y, S * 0.44, S * 0.385, S * 0.19, S * 0.145, S * 0.05);
    px = over(px, WHITE[0], WHITE[1], WHITE[2], 0.3 * cover(sdBack, 1.5));

    // 前卡投影（宽 feather 模拟模糊）
    const sdShadow = sdRoundBox(x, y, S * 0.52, S * 0.6, S * 0.2, S * 0.155, S * 0.07);
    px = over(px, SHADOW[0], SHADOW[1], SHADOW[2], 0.4 * cover(sdShadow, 26));

    // 前卡（实心白）
    const sdFront = sdRoundBox(x, y, S * 0.52, S * 0.565, S * 0.2, S * 0.155, S * 0.06);
    px = over(px, WHITE[0], WHITE[1], WHITE[2], 0.97 * cover(sdFront, 1.5));

    // 前卡上的两条对话线
    const sdL1 = sdRoundBox(x, y, S * 0.52, S * 0.52, S * 0.115, S * 0.017, S * 0.017);
    px = over(px, LINE[0], LINE[1], LINE[2], 0.95 * cover(sdL1, 1.5));
    const sdL2 = sdRoundBox(x, y, S * 0.475, S * 0.6, S * 0.07, S * 0.017, S * 0.017);
    px = over(px, LINE[0], LINE[1], LINE[2], 0.95 * cover(sdL2, 1.5));

    raw[o++] = Math.round(px[0]);
    raw[o++] = Math.round(px[1]);
    raw[o++] = Math.round(px[2]);
    raw[o++] = Math.round(px[3] * 255);
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8;
ihdr[9] = 6;
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync("src-tauri/icons", { recursive: true });
writeFileSync("app-icon.png", png);
console.log("app-icon.png written,", png.length, "bytes");
