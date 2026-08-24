#!/usr/bin/env node
'use strict';
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const directory = require('./directory');
const render = require('./render');

const {basename, join} = path;

// Every read-modify-write cycle (create/check/begin/star/priority/edit/move/
// delete/restore) has to hold this lock for its full duration, otherwise two
// processes touching the same taskbook dir at once can silently clobber each
// other's writes -- last one to save wins, the other's change just vanishes.
// No dependency: `wx` is an atomic exclusive-create (POSIX O_CREAT|O_EXCL),
// the same primitive tools like git use for their own `.lock` files.
const LOCK_ACQUIRE_TIMEOUT_MS = 5000;
const LOCK_RETRY_DELAY_MS = 50;
const LOCK_STALE_MS = 30_000;

// Tracks which lock files *this process* currently holds, shared across
// every Storage instance (not just `this`) so that e.g. `clear()` calling
// into `deleteItems()` -- two Storage-owning call paths in the same process
// -- can't deadlock waiting on a lock it already holds itself.
const heldLocks = new Set();

function releaseLockFileIfOwned(lockFile) {
  try {
    const heldBy = fs.readFileSync(lockFile, 'utf8').trim();
    if (heldBy === String(process.pid)) {
      fs.unlinkSync(lockFile);
    }
  } catch {
    // Already gone; nothing left to release.
  }
}

// One process-wide handler releases every lock this process holds, however
// many Storage instances (however many taskbook dirs) asked for one -- not
// one handler per instance, which would trip Node's max-listeners warning
// for any process that creates more than a handful (a test suite, say).
let exitHandlerRegistered = false;
function registerExitHandlerOnce() {
  if (exitHandlerRegistered) {
    return;
  }

  exitHandlerRegistered = true;
  process.on('exit', () => {
    for (const lockFile of heldLocks) {
      releaseLockFileIfOwned(lockFile);
    }

    heldLocks.clear();
  });
}

class Storage {
  constructor(options = {}) {
    this._mainAppDir = directory.retrieveTaskbookDirectory(options);
    this._storageDir = join(this._mainAppDir, 'storage');
    this._archiveDir = join(this._mainAppDir, 'archive');
    this._tempDir = join(this._mainAppDir, '.temp');
    this._archiveFile = join(this._archiveDir, 'archive.json');
    this._mainStorageFile = join(this._storageDir, 'storage.json');
    this._lockFile = join(this._mainAppDir, '.lock');

    this._ensureDirectories();
  }

  _ensureMainAppDir() {
    if (!fs.existsSync(this._mainAppDir)) {
      fs.mkdirSync(this._mainAppDir);
    }
  }

  _ensureStorageDir() {
    if (!fs.existsSync(this._storageDir)) {
      fs.mkdirSync(this._storageDir);
    }
  }

  _ensureTempDir() {
    if (!fs.existsSync(this._tempDir)) {
      fs.mkdirSync(this._tempDir);
    }
  }

  _ensureArchiveDir() {
    if (!fs.existsSync(this._archiveDir)) {
      fs.mkdirSync(this._archiveDir);
    }
  }

  _cleanTempDir() {
    const tempFiles = fs.readdirSync(this._tempDir).map(x => join(this._tempDir, x));

    if (tempFiles.length !== 0) {
      tempFiles.forEach(tempFile => fs.unlinkSync(tempFile));
    }
  }

  _ensureDirectories() {
    this._ensureMainAppDir();
    this._ensureStorageDir();
    this._ensureArchiveDir();
    this._ensureTempDir();
    this._cleanTempDir();
  }

  _isProcessAlive(pid) {
    try {
      process.kill(pid, 0);
      return true;
    } catch (error) {
      return error.code !== 'ESRCH';
    }
  }

  _blockingSleep(ms) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
  }

  _clearLockIfStale() {
    let stats;
    let content;

    try {
      stats = fs.statSync(this._lockFile);
      content = fs.readFileSync(this._lockFile, 'utf8').trim();
    } catch {
      return true;
    }

    const pid = Number(content);
    const age = Date.now() - stats.mtimeMs;
    const heldByLiveProcess = Number.isInteger(pid) && this._isProcessAlive(pid);

    if (heldByLiveProcess && age < LOCK_STALE_MS) {
      return false;
    }

    try {
      fs.unlinkSync(this._lockFile);
    } catch {
      // Already cleared, possibly by whoever held it. Either way, gone.
    }

    return true;
  }

  acquireLock() {
    if (heldLocks.has(this._lockFile)) {
      return;
    }

    const deadline = Date.now() + LOCK_ACQUIRE_TIMEOUT_MS;

    for (;;) {
      try {
        const fd = fs.openSync(this._lockFile, 'wx');
        fs.writeSync(fd, String(process.pid));
        fs.closeSync(fd);
        break;
      } catch (error) {
        if (error.code !== 'EEXIST') {
          throw error;
        }

        if (this._clearLockIfStale()) {
          continue;
        }

        if (Date.now() >= deadline) {
          render.lockTimeout(this._lockFile);
          process.exit(1);
        }

        this._blockingSleep(LOCK_RETRY_DELAY_MS);
      }
    }

    heldLocks.add(this._lockFile);
    registerExitHandlerOnce();
  }

  releaseLock() {
    if (!heldLocks.has(this._lockFile)) {
      return;
    }

    releaseLockFileIfOwned(this._lockFile);
    heldLocks.delete(this._lockFile);
  }

  _getRandomHexString(length = 8) {
    return crypto.randomBytes(Math.ceil(length / 2)).toString('hex').slice(0, length);
  }

  _getTempFile(filePath) {
    const randomString = this._getRandomHexString();
    const tempFilename = basename(filePath).split('.').join(`.TEMP-${randomString}.`);
    return join(this._tempDir, tempFilename);
  }

  get() {
    let data = {};

    if (fs.existsSync(this._mainStorageFile)) {
      const content = fs.readFileSync(this._mainStorageFile, 'utf8');
      data = JSON.parse(content);
    }

    return data;
  }

  getArchive() {
    let archive = {};

    if (fs.existsSync(this._archiveFile)) {
      const content = fs.readFileSync(this._archiveFile, 'utf8');
      archive = JSON.parse(content);
    }

    return archive;
  }

  set(data) {
    data = JSON.stringify(data, null, 4);
    const tempStorageFile = this._getTempFile(this._mainStorageFile);

    fs.writeFileSync(tempStorageFile, data, 'utf8');
    fs.renameSync(tempStorageFile, this._mainStorageFile);
  }

  setArchive(archive) {
    const data = JSON.stringify(archive, null, 4);
    const tempArchiveFile = this._getTempFile(this._archiveFile);

    fs.writeFileSync(tempArchiveFile, data, 'utf8');
    fs.renameSync(tempArchiveFile, this._archiveFile);
  }
}

module.exports = Storage;
