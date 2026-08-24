#!/usr/bin/env node
'use strict';
const clipboardy = require('clipboardy');
const Task = require('./task');
const Note = require('./note');
const Storage = require('./storage');
const render = require('./render');

class Taskbook {
  constructor(options = {}) {
    this._json = Boolean(options.json);
    render.setJsonMode(this._json);
    this._storage = new Storage(options);
  }

  get _archive() {
    return this._storage.getArchive();
  }

  get _data() {
    return this._storage.get();
  }

  _arrayify(x) {
    return Array.isArray(x) ? x : [x];
  }

  _save(data) {
    this._storage.set(data);
  }

  _saveArchive(data) {
    this._storage.setArchive(data);
  }

  _removeDuplicates(x) {
    return [...new Set(this._arrayify(x))];
  }

  _generateID(data = this._data) {
    const ids = Object.keys(data).map(id => parseInt(id, 10));
    const max = (ids.length === 0) ? 0 : Math.max(...ids);
    return max + 1;
  }

  _validateIDs(inputIDs, existingIDs = this._getIDs()) {
    if (inputIDs.length === 0) {
      render.missingID();
      process.exit(1);
    }

    inputIDs = this._removeDuplicates(inputIDs);

    inputIDs.forEach(id => {
      if (existingIDs.indexOf(Number(id)) === -1) {
        render.invalidID(id);
        process.exit(1);
      }
    });

    return inputIDs;
  }

  _isPriorityOpt(x) {
    return ['p:1', 'p:2', 'p:3'].indexOf(x) > -1;
  }

  _getBoards() {
    const {_data} = this;
    const boards = ['My Board'];

    Object.keys(_data).forEach(id => {
      boards.push(..._data[id].boards.filter(x => boards.indexOf(x) === -1));
    });

    return boards;
  }

  _getDates(data = this._data) {
    const dates = [];

    Object.keys(data).forEach(id => {
      if (dates.indexOf(data[id]._date) === -1) {
        dates.push(data[id]._date);
      }
    });

    return dates;
  }

  _getIDs(data = this._data) {
    return Object.keys(data).map(id => parseInt(id, 10));
  }

  _getPriority(desc) {
    const opt = desc.find(x => this._isPriorityOpt(x));
    return opt ? opt[opt.length - 1] : 1;
  }

  _getOptions(input) {
    const [boards, desc] = [[], []];

    if (input.length === 0) {
      render.missingDesc();
      process.exit(1);
    }

    const id = this._generateID();
    const priority = this._getPriority(input);

    input.forEach(x => {
      if (!this._isPriorityOpt(x)) {
        return x.startsWith('@') && x.length > 1 ? boards.push(x) : desc.push(x);
      }
    });

    const description = desc.join(' ');

    if (description.length === 0) {
      render.missingDesc();
      process.exit(1);
    }

    if (boards.length === 0) {
      boards.push('My Board');
    }

    return {boards, description, id, priority};
  }

  _getStats() {
    const {_data} = this;
    let [complete, inProgress, pending, notes] = [0, 0, 0, 0];

    Object.keys(_data).forEach(id => {
      if (_data[id]._isTask) {
        return _data[id].isComplete ? complete++ : _data[id].inProgress ? inProgress++ : pending++;
      }

      return notes++;
    });

    const total = complete + pending + inProgress;
    const percent = (total === 0) ? 0 : Math.floor(complete * 100 / total);

    return {percent, complete, inProgress, pending, notes};
  }

  _hasTerms(string, terms) {
    for (const term of terms) {
      if (string.toLocaleLowerCase().indexOf(term.toLocaleLowerCase()) > -1) {
        return string;
      }
    }
  }

  _filterTask(data) {
    Object.keys(data).forEach(id => {
      if (!data[id]._isTask) {
        delete data[id];
      }
    });
    return data;
  }

  _filterStarred(data) {
    Object.keys(data).forEach(id => {
      if (!data[id].isStarred) {
        delete data[id];
      }
    });
    return data;
  }

  _filterInProgress(data) {
    Object.keys(data).forEach(id => {
      if (!data[id]._isTask || !data[id].inProgress) {
        delete data[id];
      }
    });
    return data;
  }

  _filterComplete(data) {
    Object.keys(data).forEach(id => {
      if (!data[id]._isTask || !data[id].isComplete) {
        delete data[id];
      }
    });
    return data;
  }

  _filterPending(data) {
    Object.keys(data).forEach(id => {
      if (!data[id]._isTask || data[id].isComplete) {
        delete data[id];
      }
    });
    return data;
  }

  _filterNote(data) {
    Object.keys(data).forEach(id => {
      if (data[id]._isTask) {
        delete data[id];
      }
    });
    return data;
  }

  _filterByAttributes(attr, data = this._data) {
    if (Object.keys(data).length === 0) {
      return data;
    }

    attr.forEach(x => {
      switch (x) {
        case 'star':
        case 'starred':
          data = this._filterStarred(data);
          break;

        case 'done':
        case 'checked':
        case 'complete':
          data = this._filterComplete(data);
          break;

        case 'progress':
        case 'started':
        case 'begun':
          data = this._filterInProgress(data);
          break;

        case 'pending':
        case 'unchecked':
        case 'incomplete':
          data = this._filterPending(data);
          break;

        case 'todo':
        case 'task':
        case 'tasks':
          data = this._filterTask(data);
          break;

        case 'note':
        case 'notes':
          data = this._filterNote(data);
          break;

        default:
          break;
      }
    });

    return data;
  }

  _groupByBoard(data = this._data, boards = this._getBoards()) {
    const grouped = {};

    if (boards.length === 0) {
      boards = this._getBoards();
    }

    Object.keys(data).forEach(id => {
      boards.forEach(board => {
        if (data[id].boards.includes(board)) {
          if (Array.isArray(grouped[board])) {
            return grouped[board].push(data[id]);
          }

          grouped[board] = [data[id]];
          return grouped[board];
        }
      });
    });

    return grouped;
  }

  _groupByDate(data = this._data, dates = this._getDates()) {
    const grouped = {};

    Object.keys(data).forEach(id => {
      dates.forEach(date => {
        if (data[id]._date === date) {
          if (Array.isArray(grouped[date])) {
            return grouped[date].push(data[id]);
          }

          grouped[date] = [data[id]];
          return grouped[date];
        }
      });
    });

    return grouped;
  }

  _saveItemToArchive(item) {
    const {_archive} = this;
    const archiveID = this._generateID(_archive);

    item._id = archiveID;
    _archive[archiveID] = item;

    this._saveArchive(_archive);
  }

  _saveItemToStorage(item) {
    const {_data} = this;
    const restoreID = this._generateID();

    item._id = restoreID;
    _data[restoreID] = item;

    this._save(_data);
  }

  createNote(desc) {
    this._storage.acquireLock();
    const {id, description, boards} = this._getOptions(desc);
    const note = new Note({id, description, boards});
    const {_data} = this;
    _data[id] = note;
    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'note', item: note});
    }

    render.successCreate(note);
  }

  copyToClipboard(ids) {
    ids = this._validateIDs(ids);
    const {_data} = this;
    const descriptions = [];

    ids.forEach(id => descriptions.push(_data[id].description));

    clipboardy.writeSync(descriptions.join('\n'));

    if (this._json) {
      return render.json({ok: true, command: 'copy', ids: ids.map(Number), descriptions});
    }

    render.successCopyToClipboard(ids);
  }

  checkTasks(ids) {
    this._storage.acquireLock();
    ids = this._validateIDs(ids);
    const {_data} = this;
    const [checked, unchecked] = [[], []];

    ids.forEach(id => {
      if (_data[id]._isTask) {
        _data[id].inProgress = false;
        _data[id].isComplete = !_data[id].isComplete;
        return _data[id].isComplete ? checked.push(id) : unchecked.push(id);
      }
    });

    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'check', checked: checked.map(Number), unchecked: unchecked.map(Number)});
    }

    render.markComplete(checked);
    render.markIncomplete(unchecked);
  }

  beginTasks(ids) {
    this._storage.acquireLock();
    ids = this._validateIDs(ids);
    const {_data} = this;
    const [started, paused] = [[], []];

    ids.forEach(id => {
      if (_data[id]._isTask) {
        _data[id].isComplete = false;
        _data[id].inProgress = !_data[id].inProgress;
        return _data[id].inProgress ? started.push(id) : paused.push(id);
      }
    });

    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'begin', started: started.map(Number), paused: paused.map(Number)});
    }

    render.markStarted(started);
    render.markPaused(paused);
  }

  createTask(desc) {
    this._storage.acquireLock();
    const {boards, description, id, priority} = this._getOptions(desc);
    const task = new Task({id, description, boards, priority});
    const {_data} = this;
    _data[id] = task;
    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'task', item: task});
    }

    render.successCreate(task);
  }

  deleteItems(ids) {
    this._storage.acquireLock();
    ids = this._validateIDs(ids);
    const {_data} = this;
    const archived = [];

    ids.forEach(id => {
      const item = _data[id];
      this._saveItemToArchive(item);
      archived.push({storageId: Number(id), archiveId: item._id});
      delete _data[id];
    });

    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'delete', items: archived});
    }

    render.successDelete(ids);
  }

  displayArchive() {
    const grouped = this._groupByDate(this._archive, this._getDates(this._archive));

    if (this._json) {
      return render.json({ok: true, command: 'archive', dates: grouped});
    }

    render.displayByDate(grouped);
  }

  displayByBoard() {
    const grouped = this._groupByBoard();

    if (this._json) {
      return render.json({ok: true, command: 'board', boards: grouped});
    }

    render.displayByBoard(grouped);
  }

  displayByDate() {
    const grouped = this._groupByDate();

    if (this._json) {
      return render.json({ok: true, command: 'timeline', dates: grouped});
    }

    render.displayByDate(grouped);
  }

  displayStats() {
    const stats = this._getStats();

    if (this._json) {
      return render.json({ok: true, command: 'stats', stats});
    }

    render.displayStats(stats);
  }

  editDescription(input) {
    this._storage.acquireLock();
    const targets = input.filter(x => x.startsWith('@'));

    if (targets.length === 0) {
      render.missingID();
      process.exit(1);
    }

    if (targets.length > 1) {
      render.invalidIDsNumber();
      process.exit(1);
    }

    const [target] = targets;
    const id = this._validateIDs(target.replace('@', ''));
    const newDesc = input.filter(x => x !== target).join(' ');

    if (newDesc.length === 0) {
      render.missingDesc();
      process.exit(1);
    }

    const {_data} = this;
    _data[id].description = newDesc;
    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'edit', item: _data[id]});
    }

    render.successEdit(id);
  }

  findItems(terms) {
    const result = {};
    const {_data} = this;

    Object.keys(_data).forEach(id => {
      if (!this._hasTerms(_data[id].description, terms)) {
        return;
      }

      result[id] = _data[id];
    });

    const grouped = this._groupByBoard(result);

    if (this._json) {
      return render.json({ok: true, command: 'find', boards: grouped});
    }

    render.displayByBoard(grouped);
  }

  listByAttributes(terms) {
    let [boards, attributes] = [[], []];
    const storedBoards = this._getBoards();

    terms.forEach(x => {
      if (storedBoards.indexOf(`@${x}`) === -1) {
        return x === 'myboard' ? boards.push('My Board') : attributes.push(x);
      }

      return boards.push(`@${x}`);
    });

    [boards, attributes] = [boards, attributes].map(x => this._removeDuplicates(x));

    const data = this._filterByAttributes(attributes);
    const grouped = this._groupByBoard(data, boards);

    if (this._json) {
      return render.json({ok: true, command: 'list', boards: grouped});
    }

    render.displayByBoard(grouped);
  }

  moveBoards(input) {
    this._storage.acquireLock();
    let boards = [];
    const targets = input.filter(x => x.startsWith('@'));

    if (targets.length === 0) {
      render.missingID();
      process.exit(1);
    }

    if (targets.length > 1) {
      render.invalidIDsNumber();
      process.exit(1);
    }

    const [target] = targets;
    const id = this._validateIDs(target.replace('@', ''));

    input.filter(x => x !== target).forEach(x => {
      boards.push(x === 'myboard' ? 'My Board' : `@${x}`);
    });

    if (boards.length === 0) {
      render.missingBoards();
      process.exit(1);
    }

    boards = this._removeDuplicates(boards);

    const {_data} = this;
    _data[id].boards = boards;
    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'move', item: _data[id]});
    }

    render.successMove(id, boards);
  }

  restoreItems(ids) {
    this._storage.acquireLock();
    ids = this._validateIDs(ids, this._getIDs(this._archive));
    const {_archive} = this;
    const restored = [];

    ids.forEach(id => {
      const item = _archive[id];
      this._saveItemToStorage(item);
      restored.push({archiveId: Number(id), storageId: item._id});
      delete _archive[id];
    });

    this._saveArchive(_archive);

    if (this._json) {
      return render.json({ok: true, command: 'restore', items: restored});
    }

    render.successRestore(ids);
  }

  starItems(ids) {
    this._storage.acquireLock();
    ids = this._validateIDs(ids);
    const {_data} = this;
    const [starred, unstarred] = [[], []];

    ids.forEach(id => {
      _data[id].isStarred = !_data[id].isStarred;
      return _data[id].isStarred ? starred.push(id) : unstarred.push(id);
    });

    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'star', starred: starred.map(Number), unstarred: unstarred.map(Number)});
    }

    render.markStarred(starred);
    render.markUnstarred(unstarred);
  }

  updatePriority(input) {
    this._storage.acquireLock();
    const level = input.find(x => ['1', '2', '3'].indexOf(x) > -1);

    if (!level) {
      render.invalidPriority();
      process.exit(1);
    }

    const targets = input.filter(x => x.startsWith('@'));

    if (targets.length === 0) {
      render.missingID();
      process.exit(1);
    }

    if (targets.length > 1) {
      render.invalidIDsNumber();
      process.exit(1);
    }

    const [target] = targets;
    const id = this._validateIDs(target.replace('@', ''));

    const {_data} = this;
    _data[id].priority = level;
    this._save(_data);

    if (this._json) {
      return render.json({ok: true, command: 'priority', item: _data[id]});
    }

    render.successPriority(id, level);
  }

  clear() {
    const ids = [];
    const {_data} = this;

    Object.keys(_data).forEach(id => {
      if (_data[id].isComplete) {
        ids.push(id);
      }
    });

    if (ids.length === 0) {
      if (this._json) {
        return render.json({ok: true, command: 'delete', items: []});
      }

      return;
    }

    this.deleteItems(ids);
  }
}

module.exports = Taskbook;
