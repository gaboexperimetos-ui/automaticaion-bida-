const test = require('node:test');
const assert = require('node:assert/strict');
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
