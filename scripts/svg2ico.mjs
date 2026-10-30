import { readFileSync, writeFileSync } from 'node:fs';
import { Resvg } from '@resvg/resvg-js';

const resource = new URL('../crates/winlex-gui/res/', import.meta.url);
const svg = readFileSync(new URL('appicon.svg', resource));
const sizes = [256, 48, 32, 24, 16];

function bitmap(image) {
    const { width, height } = image;
    const rgba = Buffer.from(image.pixels);
    // resvg pixels are premultiplied; DIB stores straight BGRA.
    for (let i = 0; i < rgba.length; i += 4) {
        const alpha = rgba[i + 3];
        for (let channel = 0; channel < 3; channel++) {
            rgba[i + channel] = alpha === 0 ? 0
                : Math.min(255, Math.round(rgba[i + channel] * 255 / alpha));
        }
    }
    const header = Buffer.alloc(40);
    header.writeUInt32LE(40, 0);
    header.writeInt32LE(width, 4);
    header.writeInt32LE(height * 2, 8);
    header.writeUInt16LE(1, 12);
    header.writeUInt16LE(32, 14);
    const pixels = Buffer.alloc(width * height * 4);
    const stride = Math.ceil(width / 32) * 4;
    const mask = Buffer.alloc(stride * height);
    for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
            const src = ((height - 1 - y) * width + x) * 4;
            const dst = (y * width + x) * 4;
            pixels[dst] = rgba[src + 2];
            pixels[dst + 1] = rgba[src + 1];
            pixels[dst + 2] = rgba[src];
            pixels[dst + 3] = rgba[src + 3];
            if (rgba[src + 3] === 0) {
                mask[y * stride + (x >> 3)] |= 0x80 >> (x & 7);
            }
        }
    }
    return Buffer.concat([header, pixels, mask]);
}

const entries = sizes.map(size => {
    const image = new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render();
    if (image.width !== size || image.height !== size) {
        throw new Error('Application icon must have a square canvas');
    }
    return size === 256 ? image.asPng() : bitmap(image);
});
const directory = Buffer.alloc(6 + entries.length * 16);
directory.writeUInt16LE(1, 2);
directory.writeUInt16LE(entries.length, 4);
let offset = directory.length;
entries.forEach((data, i) => {
    const entry = 6 + i * 16;
    directory[entry] = directory[entry + 1] = sizes[i] % 256;
    directory.writeUInt16LE(1, entry + 4);
    directory.writeUInt16LE(32, entry + 6);
    directory.writeUInt32LE(data.length, entry + 8);
    directory.writeUInt32LE(offset, entry + 12);
    offset += data.length;
});
writeFileSync(new URL('appicon.ico', resource), Buffer.concat([directory, ...entries]));
console.log('appicon.svg -> appicon.ico');
