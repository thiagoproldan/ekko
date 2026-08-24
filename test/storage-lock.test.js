'use strict';
const {test, describe} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const Storage = require('../src/storage');
const {makeTempTaskbookDir, cleanupTempDir} = require('./helpers');

describe('storage lock', () => {
  test('acquireLock creates a lock file holding this process id; releaseLock removes it', () => {
    const dir = makeTempTaskbookDir();
    const storage = new Storage({taskbookDir: dir});
    const lockFile = path.join(dir, '.taskbook', '.lock');

    storage.acquireLock();
    assert.strictEqual(fs.readFileSync(lockFile, 'utf8'), String(process.pid));

    storage.releaseLock();
    assert.strictEqual(fs.existsSync(lockFile), false);

    cleanupTempDir(dir);
  });

  test('acquireLock is a no-op if this process already holds it (no self-deadlock)', () => {
    const dir = makeTempTaskbookDir();
    const storage = new Storage({taskbookDir: dir});

    storage.acquireLock();
    storage.acquireLock(); // Must return immediately, not hang waiting on itself.
    storage.releaseLock();

    cleanupTempDir(dir);
  });

  test('a lock left behind by a dead process is cleared automatically', () => {
    const dir = makeTempTaskbookDir();
    const storage = new Storage({taskbookDir: dir});
    const lockFile = path.join(dir, '.taskbook', '.lock');

    // A pid essentially guaranteed not to exist, standing in for a process
    // that crashed while holding the lock.
    fs.writeFileSync(lockFile, '999999999');

    const start = Date.now();
    storage.acquireLock();
    const elapsedMs = Date.now() - start;

    assert.strictEqual(fs.readFileSync(lockFile, 'utf8'), String(process.pid));
    assert.ok(elapsedMs < 1000, `expected near-instant recovery, took ${elapsedMs}ms`);

    storage.releaseLock();
    cleanupTempDir(dir);
  });

  test('releaseLock never deletes a lock file this process does not own', () => {
    const dir = makeTempTaskbookDir();
    const storage = new Storage({taskbookDir: dir});
    const lockFile = path.join(dir, '.taskbook', '.lock');

    storage.acquireLock();
    // Simulate the file having been recreated by someone else in between.
    fs.writeFileSync(lockFile, '1');
    storage.releaseLock();

    assert.strictEqual(fs.existsSync(lockFile), true);

    fs.rmSync(lockFile);
    cleanupTempDir(dir);
  });
});
