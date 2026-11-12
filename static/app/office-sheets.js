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

    if (fileInput && !fileInput.dataset.ready) {
      fileInput.dataset.ready = "true";
      fileInput.addEventListener("change", async function () {
        var file = fileInput.files && fileInput.files[0];
        workbook = null;
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
        if (!root.XLSX) {
          if (status) status.textContent = "No está disponible el lector local de Excel.";
          return;
        }
        if (status) status.textContent = "Leyendo el archivo localmente…";
        try {
          var isText = /\.(csv|tsv)$/i.test(file.name);
          var parsed = isText
            ? root.XLSX.read(await file.text(), { type: "string", FS: /\.tsv$/i.test(file.name) ? "\t" : ",", cellFormula: true })
            : root.XLSX.read(await file.arrayBuffer(), { type: "array", cellFormula: true, bookVBA: false });
          if (!parsed.SheetNames || !parsed.SheetNames.length) throw new Error("El archivo no contiene hojas legibles.");
          workbook = parsed;
          parsed.SheetNames.forEach(function (name, index) {
            var option = root.document.createElement("option");
            option.value = String(index);
            option.textContent = name;
            sheetSelect.appendChild(option);
          });
          sheetSelect.hidden = parsed.SheetNames.length < 2;
          sheetSelect.setAttribute("aria-label", "Hoja que se importará");
          if (importButton) importButton.disabled = false;
          if (status) status.textContent = parsed.SheetNames.length > 1
            ? "Archivo listo: elige una de sus " + parsed.SheetNames.length + " hojas. Importar reemplazará la cuadrícula actual."
            : "Archivo listo: " + parsed.SheetNames[0] + ". Importar reemplazará la cuadrícula actual.";
        } catch (error) {
          workbook = null;
          if (status) status.textContent = error && error.message ? error.message : "No se pudo leer el archivo.";
        }
      });
    }

    if (importButton && !importButton.dataset.ready) {
      importButton.dataset.ready = "true";
      importButton.addEventListener("click", function () {
        if (!workbook || !root.XLSX || !csvInput || !importForm) return;
        try {
          var index = Number(sheetSelect && sheetSelect.value || 0);
          var sheetName = workbook.SheetNames[index];
          if (!sheetName) throw new Error("Selecciona una hoja válida.");
          var rows = worksheetToAoa(workbook.Sheets[sheetName], root.XLSX);
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
      exportButton.addEventListener("click", function () {
        if (!root.XLSX) {
          if (status) status.textContent = "No está disponible el exportador local de Excel.";
          return;
        }
        try {
          var grid = root.document.getElementById("grid");
          var rows = gridToAoa(grid);
          var workbookOut = workbookFromAoa(rows, root.XLSX, "Hoja1");
          var name = exportButton.dataset.filename || "hoja";
          root.XLSX.writeFile(workbookOut, name + ".xlsx", { bookType: "xlsx", compression: true });
          if (status) status.textContent = "Descargada una hoja XLSX editable. Revisa el archivo antes de compartirlo.";
        } catch (error) {
          if (status) status.textContent = error && error.message ? error.message : "No se pudo exportar la hoja.";
        }
      });
    }
  }

  if (root.document) {
    if (root.document.readyState === "loading") root.document.addEventListener("DOMContentLoaded", initializeEditor);
    else initializeEditor();
  }
})(typeof window !== "undefined" ? window : globalThis);
