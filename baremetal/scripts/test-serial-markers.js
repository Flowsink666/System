const test = require('node:test');
const assert = require('node:assert/strict');
const { waitForNewMarker } = require('./serial-markers');

test('a previous task count cannot satisfy the next action', async () => {
  const serial = 'UI: view=2 allocated=0 tasks=2\nTASK: spawned count=3\n';
  await assert.rejects(waitForNewMarker(() => serial, 'tasks=2', serial.length, 15, 1), /Missing new serial marker/);
});

test('a repeated state is accepted only after a new record arrives', async () => {
  let serial = 'UI: view=2 allocated=0 tasks=2\n';
  const since = serial.length;
  const waiting = waitForNewMarker(() => serial, 'tasks=2', since, 1000, 1);
  serial += 'TASK: spawned count=3\nUI: view=2 allocated=0 tasks=2\n';
  await waiting;
});
