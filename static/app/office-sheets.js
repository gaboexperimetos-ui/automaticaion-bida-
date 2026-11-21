/*
 * Importación y exportación XLSX para NOVA Office.
 * La edición local conserva los valores y las fórmulas estándar de la hoja
 * elegida. No promete preservar estilos, gráficos, macros ni otras pestañas.
 */
(function (root) {
  "use strict";

  var MAX_WORKSHEET_CELLS = 50000;
  var MAX_IMPORT_BYTES = 10 * 1024 * 1024;
  var MAX_CSV_BYTES = 900 * 1024;
  var MAX_FORM_BODY_BYTES = 1500 * 1024;
  var XLSX_URL = "/vendor/xlsx-0.20.3.full.min.js";
  var xlsxLoadPromise = null;
  var xlsxLoadTarget = null;

  function parseDelimited(text, delimiter) {
    if (delimiter !== ',' && delimiter !== '\t') {
      throw new Error("El separador debe ser coma o tabulación.");
    }
    var source = String(text == null ? "" : text);
    if (source.charCodeAt(0) === 0xfeff) source = source.slice(1);

    var rows = [];
    var row = [];
    var field = "";
    var quoted = false;
    var afterQuote = false;
    var totalCells = 0;

    function finishRow() {
      row.push(field);
      if (totalCells + row.length > MAX_WORKSHEET_CELLS) {
        throw new Error("La hoja supera el límite de 50 000 celdas para esta importación.");
      }
      totalCells += row.length;
      rows.push(row);
      row = [];
      field = "";
      afterQuote = false;
    }

    for (var i = 0; i < source.length; i += 1) {
      var character = source.charAt(i);
      if (quoted) {
        if (character === '"') {
          if (source.charAt(i + 1) === '"') {
            field += '"';
            i += 1;
          } else {
            quoted = false;
            afterQuote = true;
          }
        } else {
          field += character;
        }
        continue;
      }

      if (afterQuote && (character === ' ' || character === '\t')) {
        field += character;
        continue;
      }
      if (afterQuote && character !== delimiter && character !== '\r' && character !== '\n') {
        throw new Error("El archivo contiene texto después de cerrar una celda entre comillas.");
      }
      afterQuote = false;

      if (character === '"') {
        if (field.length > 0) {
          throw new Error("El archivo contiene comillas en una posición no válida.");
        }
        quoted = true;
      } else if (character === delimiter) {
        if (totalCells + row.length + 1 > MAX_WORKSHEET_CELLS) {
          throw new Error("La hoja supera el límite de 50 000 celdas para esta importación.");
        }
        row.push(field);
        field = "";
      } else if (character === '\r' || character === '\n') {
        if (character === '\r' && source.charAt(i + 1) === '\n') i += 1;
        finishRow();
      } else {
        field += character;
      }
    }

    if (quoted) throw new Error("El archivo termina con una celda entre comillas sin cerrar.");
    if (field.length > 0 || row.length > 0 || afterQuote) finishRow();
    return rows;
  }

  function loadXlsx(runtime) {
    var target = runtime || root;
    if (target.XLSX) return Promise.resolve(target.XLSX);
    if (xlsxLoadPromise && xlsxLoadTarget === target) return xlsxLoadPromise;

    var doc = target.document;
    var host = doc && (doc.head || doc.body || doc.documentElement);
    if (!doc || typeof doc.createElement !== "function" || !host || typeof host.appendChild !== "function") {
      return Promise.reject(new Error("No está disponible el documento para cargar el lector local de Excel."));
    }

    var script = doc.createElement("script");
    script.src = XLSX_URL;
    script.async = true;
    var request = new Promise(function (resolve, reject) {
      script.onload = function () {
        script.onload = null;
        script.onerror = null;
        if (target.XLSX) resolve(target.XLSX);
        else reject(new Error("El lector local de Excel no se inicializó."));
      };
      script.onerror = function () {
        script.onload = null;
        script.onerror = null;
        if (typeof script.remove === "function") script.remove();
        reject(new Error("No se pudo cargar el lector local de Excel."));
      };
      host.appendChild(script);
    });
    xlsxLoadTarget = target;
    xlsxLoadPromise = request.catch(function (error) {
      if (xlsxLoadTarget === target) {
        xlsxLoadPromise = null;
        xlsxLoadTarget = null;
      }
      throw error;
    });
    return xlsxLoadPromise;
  }

  function cellText(cell) {
    if (!cell) return "";
    if (typeof cell.f === "string" && cell.f.trim()) {
      return "=" + cell.f.replace(/^=/, "");
    }
    if (cell.w !== undefined && cell.w !== null) return String(cell.w);
    if (cell.v === undefined || cell.v === null) return "";
    return String(cell.v);
  }

  function worksheetToAoa(worksheet, xlsx) {
    if (!worksheet || !worksheet["!ref"]) return [[]];
    var range = xlsx.utils.decode_range(worksheet["!ref"]);
    var rowCount = range.e.r - range.s.r + 1;
    var colCount = range.e.c - range.s.c + 1;
    if (rowCount < 1 || colCount < 1 || rowCount * colCount > MAX_WORKSHEET_CELLS) {
      throw new Error("La hoja supera el límite de 50 000 celdas para esta importación.");
    }
    var rows = [];
    for (var r = range.s.r; r <= range.e.r; r += 1) {
      var row = [];
      for (var c = range.s.c; c <= range.e.c; c += 1) {
        var address = xlsx.utils.encode_cell({ r: r, c: c });
        row.push(cellText(worksheet[address]));
      }
      rows.push(row);
    }
    return rows;
  }

  function csvCell(value) {
    var text = value == null ? "" : String(value);
    return /[\",\r\n]/.test(text) ? '"' + text.replace(/"/g, '""') + '"' : text;
  }

  function aoaToCsv(rows) {
    return (rows || []).map(function (row) {
      return (row || []).map(csvCell).join(",");
    }).join("\n");
  }

  function aoaToWorksheet(rows, xlsx) {
    var input = Array.isArray(rows) ? rows : [];
    var width = input.reduce(function (max, row) {
      return Math.max(max, Array.isArray(row) ? row.length : 0);
    }, 0);
    var height = input.length;
    if (height * width > MAX_WORKSHEET_CELLS) {
      throw new Error("La hoja supera el límite de 50 000 celdas para exportar.");
    }
    var normalized = input.map(function (row) {
      return Array.from({ length: width }, function (_, index) {
        return row && row[index] != null ? String(row[index]) : "";
      });
    });
    var worksheet = xlsx.utils.aoa_to_sheet(normalized);
    for (var r = 0; r < height; r += 1) {
      for (var c = 0; c < width; c += 1) {
        var value = normalized[r][c];
        var address = xlsx.utils.encode_cell({ r: r, c: c });
        if (value.charAt(0) === "=" && value.length > 1) {
          var cell = worksheet[address] || {};
          cell.f = value.slice(1);
          delete cell.v;
          cell.t = "n";
          worksheet[address] = cell;
        } else if (value.trim() && /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$/.test(value.trim())) {
          var number = Number(value);
          if (Number.isFinite(number)) {
            worksheet[address] = { t: "n", v: number };
          }
        }
      }
    }
    return worksheet;
  }

  function workbookFromAoa(rows, xlsx, sheetName) {
    var workbook = xlsx.utils.book_new();
    var name = String(sheetName || "Hoja1").replace(/[\\/?*:[\]]/g, " ").slice(0, 31) || "Hoja1";
    xlsx.utils.book_append_sheet(workbook, aoaToWorksheet(rows, xlsx), name);
    return workbook;
  }

  function gridToAoa(grid) {
    if (!grid) throw new Error("No se encontró la cuadrícula editable.");
    return Array.from(grid.querySelectorAll("tbody tr")).map(function (row) {
      return Array.from(row.querySelectorAll("td[contenteditable]")).map(function (cell) {
        return cell.textContent || "";
      });
    });
  }

  var api = {
    MAX_WORKSHEET_CELLS: MAX_WORKSHEET_CELLS,
    MAX_IMPORT_BYTES: MAX_IMPORT_BYTES,
    parseDelimited: parseDelimited,
    loadXlsx: loadXlsx,
    worksheetToAoa: worksheetToAoa,
    aoaToCsv: aoaToCsv,
    aoaToWorksheet: aoaToWorksheet,
    workbookFromAoa: workbookFromAoa,
    gridToAoa: gridToAoa
  };

  root.NovaOfficeXlsx = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;

  function initializeEditor() {
    var fileInput = root.document.getElementById("office-xlsx-file");
    var sheetSelect = root.document.getElementById("office-xlsx-sheet");
    var importButton = root.document.getElementById("office-xlsx-import");
    var status = root.document.getElementById("office-xlsx-status");
    var csvInput = root.document.getElementById("csv-import");
    var importForm = root.document.getElementById("office-csv-import-form");
    var exportButton = root.document.getElementById("office-xlsx-export");
    var workbook = null;
    var workbookXlsx = null;
    var selectedRows = null;
    var selectedTextSheetName = "";
    var selectionVersion = 0;

    if (fileInput && !fileInput.dataset.ready) {
      fileInput.dataset.ready = "true";
      fileInput.addEventListener("change", async function () {
        var version = ++selectionVersion;
        var file = fileInput.files && fileInput.files[0];
        workbook = null;
        workbookXlsx = null;
        selectedRows = null;
        selectedTextSheetName = "";
        if (importButton) importButton.disabled = true;
        if (sheetSelect) {
          sheetSelect.replaceChildren();
          sheetSelect.hidden = true;
        }
        if (!file) return;
        if (file.size > MAX_IMPORT_BYTES) {
          if (status) status.textContent = "El archivo supera el límite de 10 MiB.";
          return;
        }
        if (!/\.(xlsx|xls|xlsm|csv|tsv)$/i.test(file.name)) {
          if (status) status.textContent = "Selecciona un archivo XLSX, XLS, CSV o TSV.";
          return;
        }

        var isText = /\.(csv|tsv)$/i.test(file.name);
        if (status) status.textContent = isText
          ? "Leyendo el archivo localmente…"
          : "Cargando el lector local de Excel…";
        try {
          if (isText) {
            var rows = parseDelimited(await file.text(), /\.tsv$/i.test(file.name) ? '\t' : ',');
            if (version !== selectionVersion) return;
            selectedRows = rows;
            selectedTextSheetName = file.name.replace(/\.(csv|tsv)$/i, "") || "Hoja1";
            if (sheetSelect) {
              var textOption = root.document.createElement("option");
              textOption.value = "0";
              textOption.textContent = selectedTextSheetName;
              sheetSelect.appendChild(textOption);
              sheetSelect.hidden = true;
              sheetSelect.setAttribute("aria-label", "Archivo que se importará");
            }
            if (importButton) importButton.disabled = false;
            if (status) status.textContent = "Archivo listo: " + selectedTextSheetName + ". Importar reemplazará la cuadrícula actual.";
            return;
          }

          var xlsx = await loadXlsx();
          if (version !== selectionVersion) return;
          var parsed = xlsx.read(await file.arrayBuffer(), { type: "array", cellFormula: true, bookVBA: false });
          if (version !== selectionVersion) return;
          if (!parsed.SheetNames || !parsed.SheetNames.length) throw new Error("El archivo no contiene hojas legibles.");
          workbook = parsed;
          workbookXlsx = xlsx;
          parsed.SheetNames.forEach(function (name, index) {
            if (!sheetSelect) return;
            var option = root.document.createElement("option");
            option.value = String(index);
            option.textContent = name;
            sheetSelect.appendChild(option);
          });
          if (sheetSelect) {
            sheetSelect.hidden = parsed.SheetNames.length < 2;
            sheetSelect.setAttribute("aria-label", "Hoja que se importará");
          }
          if (importButton) importButton.disabled = false;
          if (status) status.textContent = parsed.SheetNames.length > 1
            ? "Archivo listo: elige una de sus " + parsed.SheetNames.length + " hojas. Importar reemplazará la cuadrícula actual."
            : "Archivo listo: " + parsed.SheetNames[0] + ". Importar reemplazará la cuadrícula actual.";
        } catch (error) {
          if (version !== selectionVersion) return;
          workbook = null;
          workbookXlsx = null;
          selectedRows = null;
          if (status) status.textContent = error && error.message ? error.message : "No se pudo leer el archivo.";
        }
      });
    }

    if (importButton && !importButton.dataset.ready) {
      importButton.dataset.ready = "true";
      importButton.addEventListener("click", function () {
        if ((selectedRows === null && (!workbook || !workbookXlsx)) || !csvInput || !importForm) return;
        try {
          var rows;
          var sheetName;
          if (selectedRows !== null) {
            rows = selectedRows;
            sheetName = selectedTextSheetName || "Hoja1";
          } else {
            var index = Number(sheetSelect && sheetSelect.value || 0);
            sheetName = workbook.SheetNames[index];
            if (!sheetName) throw new Error("Selecciona una hoja válida.");
            rows = worksheetToAoa(workbook.Sheets[sheetName], workbookXlsx);
          }
          var csv = aoaToCsv(rows);
          if (new root.Blob([csv]).size > MAX_CSV_BYTES || new URLSearchParams({ csv_data: csv }).toString().length > MAX_FORM_BODY_BYTES) {
            throw new Error("La hoja convertida supera el límite de guardado seguro. Reduce los datos y vuelve a intentarlo.");
          }
          csvInput.value = csv;
          if (status) status.textContent = "Guardando «" + sheetName + "» en la cuadrícula…";
          if (root.htmx && typeof root.htmx.trigger === "function") root.htmx.trigger(importForm, "submit");
          else importForm.requestSubmit();
        } catch (error) {
          if (status) status.textContent = error && error.message ? error.message : "No se pudo importar la hoja.";
        }
      });
    }

    if (exportButton && !exportButton.dataset.ready) {
      exportButton.dataset.ready = "true";
      exportButton.addEventListener("click", async function () {
        exportButton.disabled = true;
        if (status) status.textContent = "Cargando el exportador local de Excel…";
        try {
          var xlsx = await loadXlsx();
          var grid = root.document.getElementById("grid");
          var rows = gridToAoa(grid);
          var workbookOut = workbookFromAoa(rows, xlsx, "Hoja1");
          var name = exportButton.dataset.filename || "hoja";
          xlsx.writeFile(workbookOut, name + ".xlsx", { bookType: "xlsx", compression: true });
          if (status) status.textContent = "Descargada una hoja XLSX editable. Revisa el archivo antes de compartirlo.";
        } catch (error) {
          if (status) status.textContent = error && error.message ? error.message : "No se pudo exportar la hoja.";
        } finally {
          exportButton.disabled = false;
        }
      });
    }
  }

  if (root.document) {
    if (root.document.readyState === "loading") root.document.addEventListener("DOMContentLoaded", initializeEditor);
    else initializeEditor();
  }
})(typeof window !== "undefined" ? window : globalThis);
