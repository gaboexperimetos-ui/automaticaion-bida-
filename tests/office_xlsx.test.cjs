const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const XLSX = require('../static/vendor/xlsx-0.20.3.full.min.js');
const office = require('../static/app/office-sheets.js');

test('convierte una hoja con varias pestañas seleccionando una sin perder fórmulas', () => {
  const workbook = XLSX.utils.book_new();
  const first = XLSX.utils.aoa_to_sheet([[21, null], [13, null]]);
  first.B1 = { t: 'n', f: 'SUM(A1:A2)' };
  XLSX.utils.book_append_sheet(workbook, first, 'Notas');
  XLSX.utils.book_append_sheet(workbook, XLSX.utils.aoa_to_sheet([['otra']]), 'Otra');

  const bytes = XLSX.write(workbook, { type: 'buffer', bookType: 'xlsx' });
  const reopened = XLSX.read(bytes, { type: 'buffer', cellFormula: true });
  assert.deepEqual(office.worksheetToAoa(reopened.Sheets.Notas, XLSX), [
    ['21', '=SUM(A1:A2)'],
    ['13', ''],
  ]);
  assert.equal(reopened.SheetNames.length, 2);
});

test('lee CSV con BOM, comillas, comas, comillas escapadas y saltos de línea', () => {
  const csv = '\uFEFFNombre,Nota\r\nAda,"línea 1, con coma"\r\nBea,"dijo ""sí""\ny luego"';
  assert.deepEqual(office.parseDelimited(csv, ','), [
    ['Nombre', 'Nota'],
    ['Ada', 'línea 1, con coma'],
    ['Bea', 'dijo "sí"\ny luego'],
  ]);
});

test('lee TSV nativo con tabulaciones dentro de campos entrecomillados', () => {
  assert.deepEqual(office.parseDelimited('a\t"b\tc"\r\nd\te', '\t'), [
    ['a', 'b\tc'],
    ['d', 'e'],
  ]);
});

test('rechaza CSV con comillas incompletas, texto tras una comilla y demasiadas celdas', () => {
  assert.throws(() => office.parseDelimited('"sin cerrar', ','), /sin cerrar/);
  assert.throws(() => office.parseDelimited('"celda"texto', ','), /después de cerrar/);
  assert.throws(() => office.parseDelimited(Array(50001).fill('x').join(','), ','), /50 000 celdas/);
  assert.throws(() => office.parseDelimited('a|b', '|'), /separador/);
});

test('carga el bundle XLSX local solo cuando se solicita y comparte la carga en curso', async () => {
  const scripts = [];
  const runtime = {
    document: {
      head: {
        appendChild(script) {
          scripts.push(script);
        },
      },
      createElement(tag) {
        assert.equal(tag, 'script');
        return { remove() {} };
      },
    },
  };

  const first = office.loadXlsx(runtime);
  const second = office.loadXlsx(runtime);
  assert.equal(scripts.length, 1);
  assert.equal(scripts[0].src, '/vendor/xlsx-0.20.3.full.min.js');
  assert.equal(scripts[0].async, true);

  runtime.XLSX = XLSX;
  scripts[0].onload();
  assert.equal(await first, XLSX);
  assert.equal(await second, XLSX);
});

test('importa CSV sin inyectar ni descargar SheetJS', async () => {
  const makeElement = () => ({
    dataset: {},
    disabled: false,
    hidden: false,
    value: '',
    textContent: '',
    children: [],
    listeners: {},
    addEventListener(name, listener) {
      this.listeners[name] = listener;
    },
    replaceChildren() {
      this.children = [];
    },
    appendChild(child) {
      this.children.push(child);
      if (!this.value) this.value = child.value;
    },
    setAttribute() {},
    requestSubmit() {
      this.submitted = true;
    },
  });
  const ids = [
    'office-xlsx-file',
    'office-xlsx-sheet',
    'office-xlsx-import',
    'office-xlsx-status',
    'csv-import',
    'office-csv-import-form',
    'office-xlsx-export',
    'grid',
  ];
  const elements = Object.fromEntries(ids.map((id) => [id, makeElement()]));
  const scripts = [];
  const documentListeners = {};
  const document = {
    readyState: 'loading',
    head: {
      appendChild(script) {
        scripts.push(script);
      },
    },
    getElementById(id) {
      return elements[id] || null;
    },
    createElement() {
      return makeElement();
    },
    addEventListener(name, listener) {
      documentListeners[name] = listener;
    },
  };
  const window = { document, Blob, URLSearchParams };
  const context = vm.createContext({ window, globalThis: window, Blob, URLSearchParams });
  const source = fs.readFileSync(require.resolve('../static/app/office-sheets.js'), 'utf8');
  vm.runInContext(source, context);
  documentListeners.DOMContentLoaded();

  const fileInput = elements['office-xlsx-file'];
  const importButton = elements['office-xlsx-import'];
  fileInput.files = [{ name: 'calificaciones.csv', size: 25, text: async () => 'Nombre,Nota\nAda,9' }];
  await fileInput.listeners.change();
  assert.equal(scripts.length, 0);
  assert.equal(importButton.disabled, false);
  assert.match(elements['office-xlsx-status'].textContent, /Archivo listo/);

  importButton.listeners.click();
  assert.equal(elements['csv-import'].value, 'Nombre,Nota\nAda,9');
  assert.equal(elements['office-csv-import-form'].submitted, true);
  assert.equal(scripts.length, 0);
});

test('serializa CSV con comas, comillas y saltos de línea conforme a RFC 4180', () => {
  const csv = office.aoaToCsv([['a,b', 'dijo "sí"'], ['línea 1\nlínea 2', '']]);
  assert.equal(csv, '"a,b","dijo ""sí"""\n"línea 1\nlínea 2",');
});

test('exporta un XLSX que conserva fórmulas y valores de la cuadrícula', () => {
  const workbook = office.workbookFromAoa([
    ['21', '=SUM(A1:A2)'],
    ['13', ''],
  ], XLSX, 'Hoja1');
  const bytes = XLSX.write(workbook, { type: 'buffer', bookType: 'xlsx' });
  const reopened = XLSX.read(bytes, { type: 'buffer', cellFormula: true });
  assert.equal(reopened.SheetNames[0], 'Hoja1');
  assert.equal(reopened.Sheets.Hoja1.A1.v, 21);
  assert.equal(reopened.Sheets.Hoja1.B1.f, 'SUM(A1:A2)');
});

test('rechaza dimensiones de hoja que podrían disparar una asignación enorme', () => {
  assert.throws(
    () => office.worksheetToAoa({ '!ref': 'A1:XFD1048576' }, XLSX),
    /50 000 celdas/,
  );
});
