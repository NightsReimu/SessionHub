// 生成 SessionHub 应用图标（无第三方依赖，纯 Node + zlib 手写 PNG 编码）
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";

const SIZE = 1024;

// CRC32
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

function lerp(a, b, t) {
  return Math.round(a + (b - a) * t);
}

// 设计：深靛蓝圆角方底 + 青→紫渐变圆环 + 橙色中心点（象征“会话汇聚”）
const bg = [24, 24, 27];
const c1 = [34, 211, 238]; // cyan
const c2 = [129, 140, 248]; // indigo
const dot = [251, 146, 60]; // orange

const cx = SIZE / 2;
const ringR = SIZE * 0.31;
const ringW = SIZE * 0.075;
const dotR = SIZE * 0.13;
const corner = SIZE * 0.22;

function insideRoundedRect(x, y) {
  const rx = Math.min(x, SIZE - 1 - x);
  const ry = Math.min(y, SIZE - 1 - y);
  if (rx >= corner || ry >= corner) return true;
  const dx = corner - rx;
  const dy = corner - ry;
  return dx * dx + dy * dy <= corner * corner;
}

const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
let o = 0;
for (let y = 0; y < SIZE; y++) {
  raw[o++] = 0; // filter: none
  for (let x = 0; x < SIZE; x++) {
    let r = 0, g = 0, b = 0, a = 0;
    if (insideRoundedRect(x, y)) {
      r = bg[0]; g = bg[1]; b = bg[2]; a = 255;
      const d = Math.hypot(x - cx, y - cx);
      if (Math.abs(d - ringR) <= ringW) {
        const t = (x + y) / (2 * SIZE);
        r = lerp(c1[0], c2[0], t);
        g = lerp(c1[1], c2[1], t);
        b = lerp(c1[2], c2[2], t);
      }
      if (d <= dotR) {
        r = dot[0]; g = dot[1]; b = dot[2];
      }
    }
    raw[o++] = r; raw[o++] = g; raw[o++] = b; raw[o++] = a;
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync("src-tauri/icons", { recursive: true });
writeFileSync("app-icon.png", png);
console.log("app-icon.png written,", png.length, "bytes");
