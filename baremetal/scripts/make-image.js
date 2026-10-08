// 生成带 EFI System Partition 的 64MiB MBR/FAT32 镜像；只写输出文件，不访问实体磁盘。
"use strict";
const fs = require("node:fs");
const path = require("node:path");
const input = process.argv[2];
const output = process.argv[3];
if (!input || !output) throw new Error("Usage: node make-image.js BOOTX64.EFI mini-os.img");
const efi = fs.readFileSync(input);
if (efi.toString("ascii", 0, 2) !== "MZ") throw new Error("EFI payload is not PE/COFF");
const pe = efi.readUInt32LE(0x3c);
if (efi.toString("ascii", pe, pe + 4) !== "PE\0\0" || efi.readUInt16LE(pe + 4) !== 0x8664 || efi.readUInt16LE(pe + 24 + 68) !== 10) {
  throw new Error("Payload must be an x86_64 EFI application");
}
const sector = 512, sectors = 131072, partition = 2048, volumeSectors = sectors - partition;
const reserved = 32, fatSectors = 1024, dataSector = partition + reserved + 2 * fatSectors;
const image = Buffer.alloc(sectors * sector);
const mbr = image.subarray(0, sector);
mbr[446] = 0x80; mbr[447] = 0xfe; mbr[448] = 0xff; mbr[449] = 0xff;
mbr[450] = 0xef; mbr[451] = 0xfe; mbr[452] = 0xff; mbr[453] = 0xff;
mbr.writeUInt32LE(partition, 454); mbr.writeUInt32LE(volumeSectors, 458); mbr.writeUInt16LE(0xaa55, 510);
const boot = image.subarray(partition * sector, (partition + 1) * sector);
boot.set([0xeb, 0x58, 0x90]); boot.write("MINIOS  ", 3, "ascii");
boot.writeUInt16LE(sector, 11); boot[13] = 1; boot.writeUInt16LE(reserved, 14); boot[16] = 2;
boot[21] = 0xf8; boot.writeUInt16LE(63, 24); boot.writeUInt16LE(255, 26);
boot.writeUInt32LE(partition, 28); boot.writeUInt32LE(volumeSectors, 32);
boot.writeUInt32LE(fatSectors, 36); boot.writeUInt32LE(2, 44); boot.writeUInt16LE(1, 48); boot.writeUInt16LE(6, 50);
boot[64] = 0x80; boot[66] = 0x29; boot.writeUInt32LE(0x4d4f5331, 67);
boot.write("MINI OS    ", 71, "ascii"); boot.write("FAT32   ", 82, "ascii"); boot.writeUInt16LE(0xaa55, 510);
boot.copy(image, (partition + 6) * sector);
const info = image.subarray((partition + 1) * sector, (partition + 2) * sector);
info.writeUInt32LE(0x41615252, 0); info.writeUInt32LE(0x61417272, 484);
info.writeUInt32LE(0xffffffff, 488); info.writeUInt32LE(0xffffffff, 492); info.writeUInt32LE(0xaa550000, 508);
info.copy(image, (partition + 7) * sector);
const fat = image.subarray((partition + reserved) * sector, (partition + reserved + fatSectors) * sector);
fat.writeUInt32LE(0x0ffffff8, 0); fat.writeUInt32LE(0xffffffff, 4);
for (let cluster = 2; cluster <= 4; cluster++) fat.writeUInt32LE(0x0fffffff, cluster * 4);
const fileClusters = Math.ceil(efi.length / sector);
if (fileClusters + 5 >= fat.length / 4) throw new Error("EFI payload exceeds image capacity");
for (let index = 0; index < fileClusters; index++) fat.writeUInt32LE(index + 1 === fileClusters ? 0x0fffffff : index + 6, (index + 5) * 4);
fat.copy(image, (partition + reserved + fatSectors) * sector);
function clusterOffset(cluster) { return (dataSector + cluster - 2) * sector; }
function entry(cluster, index, name, type, first, size = 0) {
  const offset = clusterOffset(cluster) + index * 32;
  image.write(name.padEnd(11, " "), offset, 11, "ascii"); image[offset + 11] = type;
  image.writeUInt16LE(first >>> 16, offset + 20); image.writeUInt16LE(first & 0xffff, offset + 26);
  image.writeUInt32LE(size, offset + 28);
}
entry(2, 0, "EFI", 0x10, 3);
entry(3, 0, ".", 0x10, 3); entry(3, 1, "..", 0x10, 0); entry(3, 2, "BOOT", 0x10, 4);
entry(4, 0, ".", 0x10, 4); entry(4, 1, "..", 0x10, 3); entry(4, 2, "BOOTX64 EFI", 0x20, 5, efi.length);
efi.copy(image, clusterOffset(5));
fs.mkdirSync(path.dirname(path.resolve(output)), { recursive: true });
fs.writeFileSync(output, image);
console.log(`Built ${path.resolve(output)} (${image.length / 1048576} MiB), /EFI/BOOT/BOOTX64.EFI = ${efi.length} bytes`);
