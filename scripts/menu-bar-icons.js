import { writeFileSync } from "node:fs";
import { crc32, deflateSync } from "node:zlib";

/**
 * Draws the menu bar extra's template images into src-tauri/icons: menu-bar.png, the clock of the
 * app's icon, and menu-bar-alert.png, the clock with a dot at its top right, which the menu bar
 * shows while a background agent fails, is blocked, or can't start. A template is black on clear:
 * macOS uses only its alpha, drawn in the menu bar's color. 36 pixels square, which macOS shows 18
 * points tall. Run `node scripts/menu-bar-icons.js` after changing a shape.
 */
const SIZE = 36;
const STROKE = 3;
/** Samples per pixel along each axis, for smooth edges. */
const SAMPLES = 8;
const center = { x: 18, y: 18 };
const ringRadius = 13.5;
/** The hands, from 12 o'clock to the middle and on to 4 o'clock, scaled from the app's icon. */
const hands = [{ x: 18, y: 9.75 }, { x: 18, y: 18.75 }, { x: 24, y: 22.5 }];
const dot = { x: 30, y: 6, radius: 5 };
/** The clear space around the dot, so it reads apart from the ring it cuts into. */
const gap = 7.5;

const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);

/** How far `point` is from the segment from `a` to `b`. */
function segmentDistance(point, a, b) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const t = Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / (dx * dx + dy * dy)));
  return distance(point, { x: a.x + t * dx, y: a.y + t * dy });
}

function clockCovers(point) {
  if (Math.abs(distance(point, center) - ringRadius) <= STROKE / 2) return true;
  return hands.slice(1).some((end, index) => segmentDistance(point, hands[index], end) <= STROKE / 2);
}

function covers(point, alert) {
  if (!alert) return clockCovers(point);
  const fromDot = distance(point, dot);
  return fromDot <= dot.radius || (fromDot > gap && clockCovers(point));
}

/** RGBA rows of the image, black with the alpha each pixel's coverage gives it. */
function pixels(alert) {
  const data = Buffer.alloc(SIZE * SIZE * 4);
  for (let y = 0; y < SIZE; y += 1) {
    for (let x = 0; x < SIZE; x += 1) {
      let covered = 0;
      for (let row = 0; row < SAMPLES; row += 1) {
        for (let column = 0; column < SAMPLES; column += 1) {
          if (covers({ x: x + (column + 0.5) / SAMPLES, y: y + (row + 0.5) / SAMPLES }, alert)) covered += 1;
        }
      }
      data[(y * SIZE + x) * 4 + 3] = Math.round((covered / (SAMPLES * SAMPLES)) * 255);
    }
  }
  return data;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const typed = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(typed));
  return Buffer.concat([length, typed, checksum]);
}

/** A PNG of 8-bit RGBA, each row unfiltered. */
function png(rgba) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(SIZE, 0);
  header.writeUInt32BE(SIZE, 4);
  header.set([8, 6, 0, 0, 0], 8);
  const rows = [];
  for (let y = 0; y < SIZE; y += 1) rows.push(Buffer.from([0]), rgba.subarray(y * SIZE * 4, (y + 1) * SIZE * 4));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.concat(rows), { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

for (const [name, alert] of [["menu-bar.png", false], ["menu-bar-alert.png", true]]) {
  writeFileSync(new URL(`../src-tauri/icons/${name}`, import.meta.url), png(pixels(alert)));
}
