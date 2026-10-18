import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const outDir = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons");
mkdirSync(outDir, { recursive: true });

const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c;
  }
  return table;
})();

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) {
    c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length, 0);
  const typeBuffer = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuffer, data])), 0);
  return Buffer.concat([length, typeBuffer, data, crc]);
}

function encodePng(width, height, rgba) {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  return Buffer.concat([
    signature,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function mix(a, b, t) {
  return Math.round(a + (b - a) * Math.max(0, Math.min(1, t)));
}

function drawMelon(size) {
  const rgba = Buffer.alloc(size * size * 4);
  const center = size / 2;
  const radius = size * 0.47;
  const seeds = [
    [0.44, 0.4],
    [0.58, 0.46],
    [0.5, 0.6],
    [0.4, 0.62],
    [0.6, 0.64],
  ];

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = x - center;
      const dy = y - center;
      const distance = Math.hypot(dx, dy);
      const index = (y * size + x) * 4;
      if (distance > radius) {
        continue;
      }

      const edge = distance / radius;
      let r;
      let g;
      let b;

      if (edge > 0.86) {
        const t = (edge - 0.86) / 0.14;
        r = mix(61, 28, t);
        g = mix(220, 150, t);
        b = mix(132, 74, t);
      } else {
        const shade = 1 - edge * 0.22 - Math.max(0, -dy / radius) * 0.18;
        r = Math.round(mix(255, 198, edge) * shade);
        g = Math.round(mix(96, 60, edge) * shade);
        b = Math.round(mix(96, 62, edge) * shade);
        for (const [sx, sy] of seeds) {
          const seedDistance = Math.hypot(x / size - sx, y / size - sy);
          if (seedDistance < 0.012) {
            r = 34;
            g = 22;
            b = 22;
          }
        }
      }

      rgba[index] = r;
      rgba[index + 1] = g;
      rgba[index + 2] = b;
      rgba[index + 3] = 255;
    }
  }
  return rgba;
}

function encodeIco(png, size) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(1, 4);
  const entry = Buffer.alloc(16);
  entry[0] = size >= 256 ? 0 : size;
  entry[1] = size >= 256 ? 0 : size;
  entry.writeUInt16LE(1, 4);
  entry.writeUInt16LE(32, 6);
  entry.writeUInt32LE(png.length, 8);
  entry.writeUInt32LE(22, 12);
  return Buffer.concat([header, entry, png]);
}

function write(name, size) {
  const png = encodePng(size, size, drawMelon(size));
  writeFileSync(join(outDir, name), png);
  return png;
}

write("32x32.png", 32);
write("128x128.png", 128);
write("128x128@2x.png", 256);
writeFileSync(join(outDir, "icon.png"), encodePng(512, 512, drawMelon(512)));
writeFileSync(join(outDir, "icon.ico"), encodeIco(encodePng(256, 256, drawMelon(256)), 256));

console.log("Wrote icons to", outDir);
