// Generates a simple 1024x1024 shield icon PNG using only Node built-ins.
// Usage: node scripts/make-icon.mjs <out.png>
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const S = 1024;
const out = process.argv[2] ?? "icon.png";
const px = Buffer.alloc(S * S * 4);

// Shield outline: half-width as a function of y (normalized 0..1 from top of shield).
function halfWidth(t) {
  if (t < 0 || t > 1) return -1;
  if (t < 0.08) return 0.36 + 0.04 * Math.sqrt(t / 0.08); // slightly rounded top shoulders
  if (t < 0.55) return 0.4;
  const u = (t - 0.55) / 0.45; // 0..1 -> taper to point
  return 0.4 * Math.cos((u * Math.PI) / 2) ** 0.9;
}
function inside(x, y, top, bottom, scale) {
  const t = (y - top) / (bottom - top);
  const hw = halfWidth(t);
  if (hw < 0) return false;
  return Math.abs(x - 0.5) <= hw * scale;
}

const SS = 4; // supersampling
for (let j = 0; j < S; j++) {
  for (let i = 0; i < S; i++) {
    let outer = 0, inner = 0, check = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const x = (i + (sx + 0.5) / SS) / S;
        const y = (j + (sy + 0.5) / SS) / S;
        if (inside(x, y, 0.08, 0.94, 1)) outer++;
        if (inside(x, y, 0.15, 0.86, 0.8)) inner++;
        // check mark: two thick line segments
        const d1 = segDist(x, y, 0.36, 0.5, 0.46, 0.61);
        const d2 = segDist(x, y, 0.46, 0.61, 0.66, 0.38);
        if (Math.min(d1, d2) < 0.035) check++;
      }
    }
    const n = SS * SS;
    const a = outer / n;
    const k = (j * S + i) * 4;
    if (a === 0) continue;
    // base: deep indigo gradient, inner: slightly lighter, check: near-white
    const g = j / S;
    let r = 49 - 10 * g, gg = 46 - 10 * g, b = 129 + 40 * (1 - g);
    const ia = inner / n;
    r = r * (1 - ia) + (79 - 15 * g) * ia;
    gg = gg * (1 - ia) + (70 - 15 * g) * ia;
    b = b * (1 - ia) + (229 - 30 * g) * ia;
    const ca = (check / n) * ia;
    r = r * (1 - ca) + 240 * ca;
    gg = gg * (1 - ca) + 244 * ca;
    b = b * (1 - ca) + 255 * ca;
    px[k] = r; px[k + 1] = gg; px[k + 2] = b; px[k + 3] = Math.round(a * 255);
  }
}
function segDist(px_, py, ax, ay, bx, by) {
  const dx = bx - ax, dy = by - ay;
  const t = Math.max(0, Math.min(1, ((px_ - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)));
  return Math.hypot(px_ - (ax + t * dx), py - (ay + t * dy));
}

// PNG encode
const raw = Buffer.alloc((S * 4 + 1) * S);
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0;
  px.copy(raw, y * (S * 4 + 1) + 1, y * S * 4, (y + 1) * S * 4);
}
const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0); ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync(out, png);
console.log(`wrote ${out} (${png.length} bytes)`);
