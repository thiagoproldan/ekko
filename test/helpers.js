'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');

// Every test gets its own throwaway directory so tests never touch a real
// ~/.taskbook and never see each other's data. Passed straight through as
// the `taskbookDir` option, same as the CLI's --taskbook-dir flag.
function makeTempTaskbookDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'taskbook-test-'));
}

function cleanupTempDir(dir) {
  fs.rmSync(dir, {recursive: true, force: true});
}

module.exports = {makeTempTaskbookDir, cleanupTempDir};
