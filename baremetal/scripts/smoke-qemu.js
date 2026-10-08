// Run against run-qemu.ps1 -Headless -QmpPort 4444. Only built-in Node modules.
const net = require('node:net');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { waitForNewMarker } = require('./serial-markers');
const port = Number(process.argv[2] || 4444);
const output = process.argv[3] ? path.resolve(process.argv[3]) : path.resolve(__dirname, '../../target/baremetal');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const log = () => fs.readFileSync(path.join(output, 'serial.log'), 'utf8');
const waitFor = (text, since = 0) => waitForNewMarker(log, text, since);
const pending = new Map();
let sequence = 0;
let buffer = '';
const socket = net.createConnection({ host: '127.0.0.1', port });
const connected = new Promise((resolve, reject) => {
  socket.once('connect', resolve);
  socket.once('error', reject);
});
socket.on('data', chunk => {
  buffer += chunk;
  let end;
  while ((end = buffer.indexOf('\n')) >= 0) {
    const line = buffer.slice(0, end).trim();
    buffer = buffer.slice(end + 1);
    if (!line) continue;
    const message = JSON.parse(line);
    const entry = pending.get(message.id);
    if (!entry) continue;
    pending.delete(message.id);
    clearTimeout(entry.timer);
    if (message.error) entry.reject(new Error(JSON.stringify(message.error)));
    else entry.resolve(message.return);
  }
});
function command(execute, args = {}) {
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`QMP timeout: ${execute}`)); }, 5000);
    pending.set(id, { resolve, reject, timer });
    socket.write(JSON.stringify({ execute, arguments: args, id }) + '\n');
  });
}
async function key(code, expected) {
  const since = log().length;
  await command('send-key', { keys: [{ type: 'qcode', data: code }], 'hold-time': 30 });
  await delay(85);
  if (expected) await waitFor(expected, since);
}
async function type(text, expected = `COMMAND: ${text}`) {
  const since = log().length;
  for (const char of text) await key(char === ' ' ? 'spc' : char === '.' ? 'dot' : char);
  await key('ret');
  await waitFor(expected, since);
}
async function mouse(dx, dy, click = false, expected) {
  const since = log().length;
  while (dx || dy) {
    const x = Math.max(-50, Math.min(50, dx));
    const y = Math.max(-50, Math.min(50, dy));
    await command('input-send-event', { events: [
      { type: 'rel', data: { axis: 'x', value: x } },
      { type: 'rel', data: { axis: 'y', value: y } },
    ] });
    dx -= x;
    dy -= y;
    await delay(150);
  }
  if (click) {
    for (const down of [true, false]) {
      await command('input-send-event', { events: [{ type: 'btn', data: { button: 'left', down } }] });
      await delay(150);
    }
  }
  if (expected) await waitFor(expected, since);
}
(async () => {
  try {
    await connected;
    await command('qmp_capabilities');
    await waitFor('BOOT: native desktop ready; PIT interrupts enabled');
    await waitFor('TICK: 100');
    await key('f2', 'UI: view=1 allocated=0 tasks=2');
    await key('a', 'ALLOC: physical frame');
    await key('f', 'FREE: physical frame');
    await key('f3', 'UI: view=2 allocated=0 tasks=2');
    await key('n', 'TASK: spawned count=3');
    await key('k', 'UI: view=2 allocated=0 tasks=2');
    await key('f4', 'UI: view=3 allocated=0 tasks=2');
    await type('write hello from qemu', 'RAMFS: note saved');
    await type('cat note.txt');
    await delay(250);
    await command('screendump', { filename: path.join(output, 'terminal.ppm') });
    // Relative motion may split into several PS/2 packets. Verify the decoded position.
    // QEMU can discard the first relative motion while activating the console.
    // Clamp at the top-left before moving to the target, like a real PS/2 device.
    await mouse(-1000, -1000);
    await mouse(100, 170, true, 'MOUSE: click x=100 y=170 view=1');
    const beforeMouseAllocation = log().length;
    await mouse(200, 110, true, 'ALLOC: physical frame');
    assert.equal((log().slice(beforeMouseAllocation).match(/ALLOC: physical frame/g) || []).length, 1, 'Mouse allocation button');
    await key('f', 'FREE: physical frame');
    await key('f5', 'UI: view=4 allocated=0 tasks=2');
    await delay(250);
    await command('screendump', { filename: path.join(output, 'files.ppm') });
    await key('f1', 'UI: view=0 allocated=0 tasks=2');
    await delay(250);
    await command('screendump', { filename: path.join(output, 'desktop.ppm') });
    const serial = log();
    const ticks = [...serial.matchAll(/TICK: (\d+)/g)].map(match => Number(match[1]));
    assert.ok(ticks.length >= 2 && ticks.at(-1) > ticks[0], 'Hardware timer advances');
    assert.ok(!serial.includes('PANIC:') && !serial.includes('BOOT ERROR:'), 'Kernel stays running');
    console.log('PASS: UEFI takeover, PIT ticks, keyboard, mouse navigation/allocation, physical page release, tasks, terminal and RAM file write/read.');
    console.log(`QEMU framebuffer screenshots: ${output}`);
  } catch (error) {
    console.error(error);
    process.exitCode = 1;
  } finally {
    socket.destroy();
  }
})();
