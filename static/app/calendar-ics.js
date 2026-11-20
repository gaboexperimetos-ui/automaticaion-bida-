/* RFC 5545 subset parser for NOVA's local calendar importer. */
(function (root, factory) {
  "use strict";
  var api = factory();
  if (typeof module === "object" && module.exports) { module.exports = api; }
  if (root) { root.SPICS = api; }
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  var MAX_FILE_LENGTH = 5 * 1024 * 1024;
  var MAX_EVENTS = 1000;
  var DAY_INDEX = { SU: 0, MO: 1, TU: 2, WE: 3, TH: 4, FR: 5, SA: 6 };
  var ALLOWED_PROPERTIES = new Set([
    "BEGIN", "END", "UID", "DTSTAMP", "DTSTART", "DTEND", "SUMMARY",
    "DESCRIPTION", "LOCATION", "RRULE", "STATUS", "TRANSP", "CLASS",
    "SEQUENCE", "CREATED", "LAST-MODIFIED", "ORGANIZER", "ATTENDEE",
    "CATEGORIES", "URL", "PRIORITY", "RELATED-TO", "RESOURCES", "COLOR"
  ]);

  function splitOutsideQuotes(value, delimiter) {
    var parts = [];
    var start = 0;
    var quoted = false;
    var escaped = false;
    for (var i = 0; i < value.length; i += 1) {
      var ch = value[i];
      if (escaped) { escaped = false; continue; }
      if (ch === "\\") { escaped = true; continue; }
      if (ch === '"') { quoted = !quoted; continue; }
      if (ch === delimiter && !quoted) {
        parts.push(value.slice(start, i));
        start = i + 1;
      }
    }
    parts.push(value.slice(start));
    return parts;
  }

  function propertyDelimiter(line) {
    var quoted = false;
    var escaped = false;
    for (var i = 0; i < line.length; i += 1) {
      var ch = line[i];
      if (escaped) { escaped = false; continue; }
      if (ch === "\\") { escaped = true; continue; }
      if (ch === '"') { quoted = !quoted; continue; }
      if (ch === ":" && !quoted) { return i; }
    }
    return -1;
  }

  function parseProperty(line) {
    var colon = propertyDelimiter(line);
    if (colon < 1) { return null; }
    var parts = splitOutsideQuotes(line.slice(0, colon), ";");
    var name = (parts.shift() || "").trim().toUpperCase();
    if (!name) { return null; }
    var params = Object.create(null);
    parts.forEach(function (part) {
      var pair = splitOutsideQuotes(part, "=");
      if (pair.length < 2) { return; }
      var key = (pair.shift() || "").trim().toUpperCase();
      var value = pair.join("=").trim().replace(/^"|"$/g, "");
      if (key) { params[key] = value; }
    });
    return { name: name, params: params, value: line.slice(colon + 1) };
  }

  function unescapeText(value) {
    return String(value || "")
      .replace(/\\[nN]/g, "\n")
      .replace(/\\([\\,;])/g, "$1");
  }

  function pad(value, width) {
    return String(value).padStart(width, "0");
  }

  function dateString(year, month, day) {
    return pad(year, 4) + "-" + pad(month, 2) + "-" + pad(day, 2);
  }

  function wallClockMillis(year, month, day, hour, minute, second) {
    var value = new Date(0);
    value.setUTCFullYear(year, month - 1, day);
    value.setUTCHours(hour, minute, second, 0);
    return value.getTime();
  }

  function validDate(year, month, day) {
    if (month < 1 || month > 12 || day < 1 || day > 31) { return false; }
    var date = new Date(wallClockMillis(year, month, day, 0, 0, 0));
    return date.getUTCFullYear() === year && date.getUTCMonth() + 1 === month && date.getUTCDate() === day;
  }

  function localParts(value) {
    if (!Number.isFinite(value.getTime())) { throw new Error("Invalid date-time value."); }
    return {
      date: dateString(value.getFullYear(), value.getMonth() + 1, value.getDate()),
      time: pad(value.getHours(), 2) + ":" + pad(value.getMinutes(), 2)
    };
  }

  function resolveInZone(year, month, day, hour, minute, second, zone) {
    try {
      var formatter = new Intl.DateTimeFormat("en-CA", {
        timeZone: zone,
        year: "numeric", month: "2-digit", day: "2-digit",
        hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23"
      });
      var wanted = wallClockMillis(year, month, day, hour, minute, second);
      var candidate = wanted;
      for (var attempt = 0; attempt < 4; attempt += 1) {
        var values = Object.create(null);
        formatter.formatToParts(new Date(candidate)).forEach(function (part) {
          values[part.type] = part.value;
        });
        var rendered = wallClockMillis(
          Number(values.year), Number(values.month), Number(values.day),
          Number(values.hour), Number(values.minute), Number(values.second)
        );
        var difference = wanted - rendered;
        if (difference === 0) { return localParts(new Date(candidate)); }
        candidate += difference;
      }
    } catch (_) {
      throw new Error('Zona horaria desconocida o inválida: "' + zone + '".');
    }
    throw new Error('La hora local no existe en la zona horaria "' + zone + '".');
  }

  function parseDateTime(property) {
    var value = property.value.trim();
    var allDay = /^(\d{4})(\d{2})(\d{2})$/.exec(value);
    if ((property.params.VALUE || "").toUpperCase() === "DATE" || allDay) {
      if (!allDay) { throw new Error("Fecha de día completo inválida."); }
      var yearDate = Number(allDay[1]);
      var monthDate = Number(allDay[2]);
      var dayDate = Number(allDay[3]);
      if (!validDate(yearDate, monthDate, dayDate)) { throw new Error("Fecha de día completo inválida."); }
      return { date: dateString(yearDate, monthDate, dayDate) };
    }

    var match = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})?(Z|[+-]\d{4})?$/i.exec(value);
    if (!match) { throw new Error("Fecha y hora iCalendar inválida."); }
    var year = Number(match[1]);
    var month = Number(match[2]);
    var day = Number(match[3]);
    var hour = Number(match[4]);
    var minute = Number(match[5]);
    var second = Number(match[6] || "0");
    var suffix = (match[7] || "").toUpperCase();
    if (!validDate(year, month, day) || hour > 23 || minute > 59 || second > 59) {
      throw new Error("Fecha y hora iCalendar inválida.");
    }
    var zone = property.params.TZID;
    if (!suffix && !zone) {
      return { date: dateString(year, month, day), time: pad(hour, 2) + ":" + pad(minute, 2) };
    }
    var millis = wallClockMillis(year, month, day, hour, minute, second);
    if (suffix === "Z") { return localParts(new Date(millis)); }
    if (suffix) {
      var offsetHours = Number(suffix.slice(1, 3));
      var offsetMinutes = Number(suffix.slice(3, 5));
      if (offsetHours > 23 || offsetMinutes > 59) { throw new Error("Desfase UTC inválido."); }
      var sign = suffix[0] === "+" ? 1 : -1;
      return localParts(new Date(millis - sign * (offsetHours * 60 + offsetMinutes) * 60000));
    }
    return resolveInZone(year, month, day, hour, minute, second, zone);
  }

  function addDays(iso, amount) {
    var parts = iso.split("-").map(Number);
    var value = new Date(0);
    value.setUTCFullYear(parts[0], parts[1] - 1, parts[2] + amount);
    value.setUTCHours(0, 0, 0, 0);
    return dateString(value.getUTCFullYear(), value.getUTCMonth() + 1, value.getUTCDate());
  }

  function weekdayFromDate(iso) {
    var parts = iso.split("-").map(Number);
    var value = new Date(0);
    value.setUTCFullYear(parts[0], parts[1] - 1, parts[2]);
    value.setUTCHours(0, 0, 0, 0);
    return value.getUTCDay();
  }

  function parseWeeklyRule(value, fallbackDay) {
    var rule = Object.create(null);
    String(value || "").split(";").forEach(function (part) {
      var separator = part.indexOf("=");
      if (separator < 1) { throw new Error("regla de recurrencia semanal inválida"); }
      var key = part.slice(0, separator).trim().toUpperCase();
      var ruleValue = part.slice(separator + 1).trim().toUpperCase();
      if (!key || !ruleValue || Object.prototype.hasOwnProperty.call(rule, key)) {
        throw new Error("regla de recurrencia semanal inválida");
      }
      rule[key] = ruleValue;
    });
    var allowed = new Set(["FREQ", "INTERVAL", "BYDAY", "WKST"]);
    if (Object.keys(rule).some(function (key) { return !allowed.has(key); })) {
      throw new Error("recurrencias con COUNT, UNTIL u otras reglas complejas no son compatibles");
    }
    if (rule.FREQ !== "WEEKLY" || (rule.INTERVAL !== undefined && rule.INTERVAL !== "1")) {
      throw new Error("solo se admite recurrencia semanal sin intervalo o con INTERVAL=1");
    }
    if (rule.WKST !== undefined && !Object.prototype.hasOwnProperty.call(DAY_INDEX, rule.WKST)) {
      throw new Error("día WKST no compatible en la recurrencia semanal");
    }
    var days = rule.BYDAY
      ? rule.BYDAY.split(",").map(function (code) {
          if (!Object.prototype.hasOwnProperty.call(DAY_INDEX, code)) {
            throw new Error("día BYDAY no compatible en la recurrencia semanal");
          }
          return DAY_INDEX[code];
        })
      : [fallbackDay];
    return Array.from(new Set(days));
  }

  function parseEvent(properties, index, warnings) {
    function first(name) {
      return properties.find(function (property) { return property.name === name; });
    }
    var status = first("STATUS");
    if (status && status.value.trim().toUpperCase() === "CANCELLED") {
      throw new Error("evento cancelado omitido");
    }
    var recurrenceRules = properties.filter(function (property) { return property.name === "RRULE"; });
    if (first("RDATE") || first("EXDATE")) {
      throw new Error("fechas de excepción o adición de recurrencia no compatibles");
    }
    if (recurrenceRules.length > 1) {
      throw new Error("varias reglas de recurrencia no son compatibles");
    }
    var recurrenceRule = recurrenceRules[0] || null;
    var start = first("DTSTART");
    if (!start) { throw new Error("falta DTSTART"); }
    var startValue = parseDateTime(start);
    var title = unescapeText(first("SUMMARY") ? first("SUMMARY").value : "").trim();
    if (!title) { throw new Error("falta SUMMARY"); }
    var endProperty = first("DTEND");
    var endValue = endProperty ? parseDateTime(endProperty) : null;
    var allDay = startValue.time === undefined;
    if (endValue && (endValue.time === undefined) !== allDay) {
      throw new Error("DTSTART y DTEND deben ser ambos de día completo o con hora");
    }
    var endDate = endValue ? endValue.date : (allDay ? addDays(startValue.date, 1) : startValue.date);
    var endTime = endValue && endValue.time ? endValue.time : undefined;
    if (endDate < startValue.date || (allDay && endDate === startValue.date)) {
      throw new Error("DTEND debe ser posterior a DTSTART");
    }
    if (!allDay && endDate === startValue.date && endTime && endTime <= startValue.time) {
      throw new Error("DTEND debe ser posterior a DTSTART");
    }

    var uidProperty = first("UID");
    var event = {
      uid: uidProperty ? unescapeText(uidProperty.value).trim() : "",
      title: title.slice(0, 120),
      description: unescapeText(first("DESCRIPTION") ? first("DESCRIPTION").value : "").slice(0, 4000),
      location: unescapeText(first("LOCATION") ? first("LOCATION").value : "").slice(0, 256),
      startDate: startValue.date,
      endDate: endDate,
      allDay: allDay
    };
    if (startValue.time) { event.startTime = startValue.time; }
    if (endTime) { event.endTime = endTime; }

    var ignored = properties.map(function (property) { return property.name; })
      .filter(function (name) { return !ALLOWED_PROPERTIES.has(name); });
    if (ignored.length) {
      warnings.push("Evento " + index + ": se ignoraron propiedades no compatibles " + Array.from(new Set(ignored)).join(", ") + ".");
    }
    if (!recurrenceRule) { return [event]; }
    if (allDay || !startValue.time || !endTime) {
      throw new Error("la recurrencia semanal necesita DTSTART y DTEND horarios");
    }
    if (endDate !== startValue.date) {
      throw new Error("la recurrencia semanal no admite eventos de varios días");
    }
    var weeklyDays = parseWeeklyRule(recurrenceRule.value, weekdayFromDate(startValue.date));
    return weeklyDays.map(function (weeklyDay) {
      return Object.assign({}, event, { weeklyDay: weeklyDay });
    });
  }

  function parseIcs(input) {
    if (typeof input !== "string") { throw new Error("No se pudo leer el archivo iCalendar."); }
    if (input.length > MAX_FILE_LENGTH) { throw new Error("El archivo ICS supera el límite de 5 MB."); }
    var lines = input.replace(/\r\n?/g, "\n").replace(/\n[ \t]/g, "").split("\n");
    if (!lines.some(function (line) { return line.trim().toUpperCase() === "BEGIN:VCALENDAR"; }) ||
        !lines.some(function (line) { return line.trim().toUpperCase() === "END:VCALENDAR"; })) {
      throw new Error("El archivo no es un iCalendar completo (VCALENDAR).");
    }

    var events = [];
    var skipped = [];
    var warnings = [];
    var current = null;
    var inAlarm = false;
    var sourceCount = 0;
    var malformed = false;
    lines.forEach(function (rawLine) {
      var line = rawLine.replace(/[\t ]+$/, "");
      if (!line) { return; }
      var upper = line.trim().toUpperCase();
      if (upper === "BEGIN:VEVENT") {
        if (current) { skipped.push("Evento " + sourceCount + ": VEVENT anidado mal formado."); }
        current = [];
        sourceCount += 1;
        if (sourceCount > MAX_EVENTS) { throw new Error("El archivo contiene más de 1000 eventos."); }
        inAlarm = false;
        malformed = false;
        return;
      }
      if (upper === "END:VEVENT") {
        if (!current) { return; }
        if (malformed) {
          skipped.push("Evento " + sourceCount + ": línea de propiedad mal formada.");
        } else {
          try {
            var parsedEvents = parseEvent(current, sourceCount, warnings);
            if (events.length + parsedEvents.length > MAX_EVENTS) {
              throw new Error("El archivo contiene más de 1000 eventos importables.");
            }
            events.push.apply(events, parsedEvents);
          } catch (error) {
            skipped.push("Evento " + sourceCount + ": " + (error && error.message ? error.message : "evento no compatible") + ".");
          }
        }
        current = null;
        if (events.length > MAX_EVENTS) { throw new Error("El archivo contiene más de 1000 eventos."); }
        return;
      }
      if (!current) { return; }
      if (upper === "BEGIN:VALARM") { inAlarm = true; return; }
      if (upper === "END:VALARM") { inAlarm = false; return; }
      if (inAlarm) { return; }
      if (!parseProperty(line)) { malformed = true; return; }
      current.push(parseProperty(line));
    });
    if (current) { skipped.push("Evento " + sourceCount + ": falta END:VEVENT."); }
    if (sourceCount === 0) { throw new Error("El archivo iCalendar no contiene registros VEVENT."); }
    return { events: events, skipped: skipped, warnings: warnings };
  }

  function identityKeys(event) {
    var weeklyDay = event.weeklyDay != null ? event.weeklyDay : event.weekly_day;
    var title = String(event.title || "").trim().toLocaleLowerCase();
    var startTime = event.startTime || event.start_time || "";
    var endTime = event.endTime || event.end_time || "";
    if (weeklyDay != null) {
      return [["weekly", title, weeklyDay, startTime, endTime].join("|")];
    }
    var keys = [];
    if (event.uid) { keys.push("uid|" + String(event.uid).trim().toLocaleLowerCase()); }
    var startDate = event.startDate || event.date || "";
    var endDate = event.endDate || event.end_date || startDate;
    var allDay = event.allDay !== undefined ? event.allDay : event.all_day !== false;
    keys.push([
      "content",
      String(event.title || "").trim().toLocaleLowerCase(),
      startDate,
      allDay ? "all-day" : (event.startTime || event.start_time || ""),
      endDate,
      event.endTime || event.end_time || ""
    ].join("|"));
    return keys;
  }

  return { parseIcs: parseIcs, identityKeys: identityKeys };
});
