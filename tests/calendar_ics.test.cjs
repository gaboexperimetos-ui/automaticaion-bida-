const test = require('node:test');
const assert = require('node:assert/strict');
const { parseIcs, identityKeys } = require('../static/app/calendar-ics.js');

function calendar(...events) {
  return ['BEGIN:VCALENDAR', 'VERSION:2.0', ...events, 'END:VCALENDAR', ''].join('\r\n');
}

test('importa eventos de día completo, campos TEXT y líneas plegadas', () => {
  const title = 'Revisión final de cálculo diferencial';
  const folded = title.slice(0, 24) + '\r\n ' + title.slice(24);
  const ics = calendar(
    'BEGIN:VEVENT',
    'UID:final-2026@example.test',
    'DTSTART;VALUE=DATE:20261020',
    'DTEND;VALUE=DATE:20261022',
    'SUMMARY:' + folded,
    'DESCRIPTION:Primera línea\\nSegunda línea',
    'LOCATION:Campus\\, Quito',
    'END:VEVENT',
  );
  const result = parseIcs(ics);
  assert.equal(result.events.length, 1);
  assert.deepEqual(result.events[0], {
    uid: 'final-2026@example.test',
    title,
    description: 'Primera línea\nSegunda línea',
    location: 'Campus, Quito',
    startDate: '2026-10-20',
    endDate: '2026-10-22',
    allDay: true,
  });
  assert.deepEqual(result.skipped, []);
});

test('importa fecha-hora flotante y zona UTC sin inventar una zona en las flotantes', () => {
  const ics = calendar(
    'BEGIN:VEVENT',
    'UID:timed@example.test',
    'DTSTART:20261021T083000',
    'DTEND:20261021T094500',
    'SUMMARY:Reunión',
    'END:VEVENT',
    'BEGIN:VEVENT',
    'DTSTART:20261022T120000Z',
    'SUMMARY:Evento UTC',
    'END:VEVENT',
  );
  const result = parseIcs(ics);
  assert.equal(result.events.length, 2);
  assert.equal(result.events[0].startTime, '08:30');
  assert.equal(result.events[0].endTime, '09:45');
  assert.equal(result.events[0].allDay, false);
  assert.equal(result.events[1].allDay, false);
  assert.match(result.events[1].startDate, /^\d{4}-\d{2}-\d{2}$/);
  assert.match(result.events[1].startTime, /^\d{2}:\d{2}$/);
});

test('importa recurrencias semanales simples por cada día BYDAY', () => {
  const result = parseIcs(calendar(
    'BEGIN:VEVENT',
    'UID:tutoria@example.test',
    'DTSTART:20261021T090000',
    'DTEND:20261021T100000',
    'SUMMARY:Tutoría',
    'LOCATION:Quito',
    'RRULE:FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;WKST=MO',
    'END:VEVENT',
  ));
  assert.equal(result.events.length, 2);
  assert.deepEqual(result.events.map((event) => event.weeklyDay), [1, 3]);
  assert.equal(result.events[0].startDate, '2026-10-21');
  assert.equal(result.events[0].startTime, '09:00');
  assert.equal(result.events[0].endTime, '10:00');
  assert.equal(result.events[0].uid, 'tutoria@example.test');
  assert.equal(identityKeys(result.events[0])[0], 'weekly|tutoría|1|09:00|10:00');
  assert.equal(identityKeys(result.events[1])[0], 'weekly|tutoría|3|09:00|10:00');
  assert.deepEqual(result.skipped, []);
});

test('usa el día de DTSTART cuando la regla semanal no declara BYDAY', () => {
  const result = parseIcs(calendar(
    'BEGIN:VEVENT',
    'DTSTART:20261025T083000',
    'DTEND:20261025T093000',
    'SUMMARY:Sesión dominical',
    'RRULE:FREQ=WEEKLY',
    'END:VEVENT',
  ));
  assert.equal(result.events.length, 1);
  assert.equal(result.events[0].weeklyDay, 0);
});

test('rechaza recurrencias acotadas, excepciones y eventos semanales de varios días', () => {
  const result = parseIcs(calendar(
    'BEGIN:VEVENT',
    'DTSTART:20261021T090000',
    'DTEND:20261021T100000',
    'SUMMARY:Serie con límite',
    'RRULE:FREQ=WEEKLY;COUNT=6',
    'END:VEVENT',
    'BEGIN:VEVENT',
    'DTSTART:20261021T090000',
    'DTEND:20261021T100000',
    'SUMMARY:Serie con excepción',
    'RRULE:FREQ=WEEKLY',
    'EXDATE:20261028T090000',
    'END:VEVENT',
    'BEGIN:VEVENT',
    'DTSTART:20261021T230000',
    'DTEND:20261022T010000',
    'SUMMARY:Serie de varios días',
    'RRULE:FREQ=WEEKLY',
    'END:VEVENT',
  ));
  assert.equal(result.events.length, 0);
  assert.equal(result.skipped.length, 3);
  assert.match(result.skipped[0], /COUNT|complejas/i);
  assert.match(result.skipped[1], /excepción/i);
  assert.match(result.skipped[2], /varios días/i);
});

test('omite recurrencias no representables, eventos cancelados y VEVENT inválidos', () => {
  const result = parseIcs(calendar(
    'BEGIN:VEVENT',
    'DTSTART;VALUE=DATE:20261020',
    'SUMMARY:Cada semana',
    'RRULE:FREQ=WEEKLY',
    'END:VEVENT',
    'BEGIN:VEVENT',
    'DTSTART;VALUE=DATE:20261020',
    'SUMMARY:Cancelado',
    'STATUS:CANCELLED',
    'END:VEVENT',
    'BEGIN:VEVENT',
    'SUMMARY:Sin fecha',
    'END:VEVENT',
  ));
  assert.equal(result.events.length, 0);
  assert.equal(result.skipped.length, 3);
  assert.match(result.skipped[0], /recurrencia/i);
  assert.match(result.skipped[1], /cancelado/i);
  assert.match(result.skipped[2], /DTSTART/i);
});

test('rechaza VCALENDAR incompleto, valores de fecha inválidos y archivos demasiado grandes', () => {
  assert.throws(() => parseIcs('BEGIN:VCALENDAR\r\nEND:VEVENT'), /completo/i);
  const badDate = calendar(
    'BEGIN:VEVENT',
    'DTSTART;VALUE=DATE:20260230',
    'SUMMARY:Fecha imposible',
    'END:VEVENT',
  );
  assert.match(parseIcs(badDate).skipped[0], /inválida/i);
  assert.throws(() => parseIcs('x'.repeat(5 * 1024 * 1024 + 1)), /5 MB/i);
});

test('las identidades detectan UID y colisiones de contenido para el preview', () => {
  const a = {
    uid: 'same@example.test', title: 'Proyecto', startDate: '2026-10-20',
    endDate: '2026-10-21', allDay: true,
  };
  const b = { ...a, title: 'Otro título' };
  assert.ok(identityKeys(a).includes('uid|same@example.test'));
  assert.ok(identityKeys(a).includes('content|proyecto|2026-10-20|all-day|2026-10-21|'));
  assert.equal(identityKeys(a)[0], identityKeys(b)[0]);
  assert.notEqual(identityKeys(a)[1], identityKeys(b)[1]);
  assert.deepEqual(identityKeys({
    title: 'Tutoría', weeklyDay: 3, startTime: '09:00', endTime: '10:00',
    uid: 'different@example.test',
  }), ['weekly|tutoría|3|09:00|10:00']);
});
