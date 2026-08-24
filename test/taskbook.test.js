'use strict';
const {test, describe, afterEach, mock} = require('node:test');
const assert = require('node:assert/strict');
const Taskbook = require('../src/taskbook');
const render = require('../src/render');
const {makeTempTaskbookDir, cleanupTempDir} = require('./helpers');

describe('_generateID', () => {
  test('starts at 1 for an empty board', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});

    assert.strictEqual(taskbook._generateID(), 1);

    cleanupTempDir(dir);
  });

  test('is max(existing ids) + 1', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});

    taskbook.createTask(['first task']);
    taskbook.createTask(['second task']);

    assert.strictEqual(taskbook._generateID(), 3);

    cleanupTempDir(dir);
  });

  test('reuses the highest id once it is deleted (not a monotonic counter)', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});

    taskbook.createTask(['a']);
    taskbook.createTask(['b']);
    taskbook.deleteItems(['2']);

    assert.strictEqual(taskbook._generateID(), 2);

    cleanupTempDir(dir);
  });
});

describe('_validateIDs', () => {
  test('passes through ids that exist, de-duplicated', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});
    taskbook.createTask(['only task']);

    const result = taskbook._validateIDs(['1', '1']);

    assert.deepStrictEqual(result, ['1']);

    cleanupTempDir(dir);
  });

  test('reports MISSING_ID and exits 1 when given no ids', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});
    const exitMock = mock.method(process, 'exit', () => {});
    const missingIDMock = mock.method(render, 'missingID', () => {});

    taskbook._validateIDs([]);

    assert.strictEqual(missingIDMock.mock.calls.length, 1);
    assert.deepStrictEqual(exitMock.mock.calls[0].arguments, [1]);

    exitMock.mock.restore();
    missingIDMock.mock.restore();
    cleanupTempDir(dir);
  });

  test('reports INVALID_ID and exits 1 for an id that does not exist', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});
    const exitMock = mock.method(process, 'exit', () => {});
    const invalidIDMock = mock.method(render, 'invalidID', () => {});

    taskbook._validateIDs(['999']);

    assert.deepStrictEqual(invalidIDMock.mock.calls[0].arguments, ['999']);
    assert.deepStrictEqual(exitMock.mock.calls[0].arguments, [1]);

    exitMock.mock.restore();
    invalidIDMock.mock.restore();
    cleanupTempDir(dir);
  });
});

// Regression coverage for the bug found and fixed while dogfooding this tool:
// `tb --task p:2` (only a priority marker, no description) used to silently
// create a task with an empty description instead of being rejected.
describe('empty description is rejected (regression)', () => {
  test('a task with only a priority marker is rejected, not silently created', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});
    const exitMock = mock.method(process, 'exit', () => {});
    const missingDescMock = mock.method(render, 'missingDesc', () => {});

    taskbook.createTask(['p:2']);

    assert.strictEqual(missingDescMock.mock.calls.length, 1);
    assert.deepStrictEqual(exitMock.mock.calls[0].arguments, [1]);

    exitMock.mock.restore();
    missingDescMock.mock.restore();
    cleanupTempDir(dir);
  });

  test('a task with only a board marker is rejected the same way', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});
    const exitMock = mock.method(process, 'exit', () => {});
    const missingDescMock = mock.method(render, 'missingDesc', () => {});

    taskbook.createNote(['@onlyaboard']);

    assert.strictEqual(missingDescMock.mock.calls.length, 1);
    assert.deepStrictEqual(exitMock.mock.calls[0].arguments, [1]);

    exitMock.mock.restore();
    missingDescMock.mock.restore();
    cleanupTempDir(dir);
  });

  test('a real description still creates the task normally', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir});

    taskbook.createTask(['@coding', 'Fix', 'the', 'bug', 'p:2']);

    const [item] = Object.values(taskbook._data);
    assert.strictEqual(item.description, 'Fix the bug');
    assert.deepStrictEqual(item.boards, ['@coding']);
    assert.strictEqual(item.priority, '2');

    cleanupTempDir(dir);
  });
});

describe('--json output', () => {
  afterEach(() => {
    render.setJsonMode(false);
  });

  test('createTask emits a structured item payload instead of pretty text', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir, json: true});
    const jsonMock = mock.method(render, 'json', () => {});

    taskbook.createTask(['@coding', 'Ship', 'it']);

    assert.strictEqual(jsonMock.mock.calls.length, 1);
    const [payload] = jsonMock.mock.calls[0].arguments;
    assert.strictEqual(payload.ok, true);
    assert.strictEqual(payload.command, 'task');
    assert.strictEqual(payload.item.description, 'Ship it');
    assert.strictEqual(payload.item._id, 1);

    jsonMock.mock.restore();
    cleanupTempDir(dir);
  });

  test('an error still exits 1 and reports a stable machine-readable code', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir, json: true});
    const jsonMock = mock.method(render, 'json', () => {});
    const exitMock = mock.method(process, 'exit', () => {});

    taskbook.checkTasks([]);

    const [payload] = jsonMock.mock.calls[0].arguments;
    assert.strictEqual(payload.ok, false);
    assert.strictEqual(payload.code, 'MISSING_ID');
    assert.deepStrictEqual(exitMock.mock.calls[0].arguments, [1]);

    exitMock.mock.restore();
    jsonMock.mock.restore();
    cleanupTempDir(dir);
  });

  test('delete reports both the storage id and the new, unrelated archive id', () => {
    const dir = makeTempTaskbookDir();
    const taskbook = new Taskbook({taskbookDir: dir, json: true});
    const jsonMock = mock.method(render, 'json', () => {});

    taskbook.createTask(['first']);
    jsonMock.mock.resetCalls();
    taskbook.deleteItems(['1']);

    const [payload] = jsonMock.mock.calls[0].arguments;
    assert.strictEqual(payload.command, 'delete');
    assert.deepStrictEqual(payload.items, [{storageId: 1, archiveId: 1}]);

    jsonMock.mock.restore();
    cleanupTempDir(dir);
  });
});
