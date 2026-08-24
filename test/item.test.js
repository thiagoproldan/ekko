'use strict';
const {test, describe} = require('node:test');
const assert = require('node:assert/strict');
const Item = require('../src/item');
const Task = require('../src/task');
const Note = require('../src/note');

// Regression coverage: `now` used to be computed once, at module require
// time, and reused for every item's `_date`/`_timestamp`. Harmless for the
// CLI (one process per command, so "module load" and "item creation" happen
// back to back), but a real bug for anything that requires this module once
// and constructs items over time -- our own test suite included.
describe('Item timestamps', () => {
  test('each item gets its own timestamp, not one frozen at module load', () => {
    const first = new Task({id: 1, description: 'first'});

    // Force the clock to tick forward before creating the second item, so
    // this fails reliably on the old, buggy behaviour instead of only
    // sometimes (two `new Date()` calls a fraction of a millisecond apart
    // could otherwise coincidentally match even without the bug).
    const start = Date.now();
    while (Date.now() === start) {
      // Busy-wait for the millisecond to roll over.
    }

    const second = new Task({id: 2, description: 'second'});

    assert.notStrictEqual(second._timestamp, first._timestamp);
  });

  test('Task and Note both pick up a fresh timestamp too', () => {
    const task = new Task({id: 1, description: 'a task'});
    const start = Date.now();
    while (Date.now() === start) {
      // Busy-wait for the millisecond to roll over.
    }

    const note = new Note({id: 2, description: 'a note'});

    assert.notStrictEqual(note._timestamp, task._timestamp);
  });

  test('_date and _timestamp reflect the same moment', () => {
    const item = new Item({id: 1, description: 'x'});
    assert.strictEqual(item._date, new Date(item._timestamp).toDateString());
  });
});
