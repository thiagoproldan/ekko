'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {spawn} = require('child_process');
const fs = require('fs');
const path = require('path');
const {makeTempTaskbookDir, cleanupTempDir} = require('./helpers');

const CLI_PATH = path.join(__dirname, '..', 'cli.js');

function runTaskbook(taskbookDir, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [CLI_PATH, '--taskbook-dir', taskbookDir, ...args]);
    child.on('error', reject);
    child.on('exit', code => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`tb ${args.join(' ')} exited ${code}`));
      }
    });
  });
}

// Regression test for the "lost update" race: before the storage lock,
// launching many `tb --task` processes against the same taskbook dir at
// once would silently drop some of them -- 25 concurrent creates surviving
// as only 19 items on disk, no error, no warning. See src/storage.js.
test('concurrent writers neither collide on an id nor lose an update', {timeout: 30_000}, async () => {
  const dir = makeTempTaskbookDir();
  const total = 15;

  const runs = Array.from({length: total}, (_, i) => runTaskbook(dir, ['--task', `concurrent task ${i}`]));
  await Promise.all(runs);

  const storagePath = path.join(dir, '.taskbook', 'storage', 'storage.json');
  const data = JSON.parse(fs.readFileSync(storagePath, 'utf8'));
  const ids = Object.keys(data).map(Number);
  const descriptions = Object.values(data).map(item => item.description);

  assert.strictEqual(ids.length, total, `expected ${total} items, found ${ids.length} (lost update)`);
  assert.strictEqual(new Set(ids).size, total, 'expected every id to be unique');
  assert.strictEqual(new Set(descriptions).size, total, 'expected every description to be unique');

  cleanupTempDir(dir);
});
