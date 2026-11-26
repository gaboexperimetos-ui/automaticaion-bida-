//! Importación de documentos locales a la biblioteca de NOVA Office.
//!
//! Convierte a los formatos nativos de la biblioteca sin depender de
//! LibreOffice ni de servicios externos:
//!   - Markdown / texto / LaTeX      -> documento Markdown (.md)
//!   - HTML                          -> página HTML (.html)
//!   - CSV / TSV / XLSX / ODS        -> hoja CSV (.csv)
//!   - DOCX / ODT                    -> documento Markdown (.md)
//!   - PPTX / ODP                    -> presentación JSON (.json)
//!   - PDF                           -> documento Markdown vía `pdftotext`
//!
//! Los OOXML/ODF se leen con `std.zip` + extracción de texto XML. Nada se
//! escribe aquí: el llamador decide el destino y el nombre final.

const std = @import("std");

pub const Kind = enum {
    docs,
    sheets,
    presentations,
    html,

    pub fn ext(self: Kind) []const u8 {
        return switch (self) {
            .docs => ".md",
            .sheets => ".csv",
            .presentations => ".json",
            .html => ".html",
        };
    }
};

pub const Converted = struct {
    kind: Kind,
    text: []const u8,
};

pub const Error = error{
    UnsupportedFormat,
    EmptyDocument,
    FileTooLarge,
    InvalidArchive,
    PdfToolMissing,
    PdfFailed,
};

const MAX_INPUT = 64 * 1024 * 1024;
const MAX_ENTRY = 48 * 1024 * 1024;
const MAX_SHEET_ROWS = 5000;
const MAX_SHEET_COLUMNS = 200;

/// Extensiones que la importación reconoce. Devuelve null si no es un
/// documento (imágenes, código, configuraciones, binarios…).
pub fn kindFor(path: []const u8) ?Kind {
    const ext = extension(path) orelse return null;
    if (eql(ext, "md") or eql(ext, "markdown") or eql(ext, "txt") or eql(ext, "text") or eql(ext, "tex") or eql(ext, "docx") or eql(ext, "odt") or eql(ext, "pdf")) return .docs;
    if (eql(ext, "csv") or eql(ext, "tsv") or eql(ext, "xlsx") or eql(ext, "ods")) return .sheets;
    if (eql(ext, "pptx") or eql(ext, "odp")) return .presentations;
    if (eql(ext, "html") or eql(ext, "htm")) return .html;
    return null;
}

pub fn convert(allocator: std.mem.Allocator, path: []const u8, display_name: []const u8) !Converted {
    const ext = extension(path) orelse return Error.UnsupportedFormat;
    if (eql(ext, "md") or eql(ext, "markdown") or eql(ext, "txt") or eql(ext, "text") or eql(ext, "tex")) {
        const raw = try readFile(allocator, path, MAX_INPUT);
        const text = try cleanText(allocator, raw);
        if (text.len == 0) return Error.EmptyDocument;
        return .{ .kind = .docs, .text = text };
    }
    if (eql(ext, "html") or eql(ext, "htm")) {
        const raw = try readFile(allocator, path, MAX_INPUT);
        if (std.mem.trim(u8, raw, " \t\r\n").len == 0) return Error.EmptyDocument;
        return .{ .kind = .html, .text = raw };
    }
    if (eql(ext, "csv")) {
        const raw = try readFile(allocator, path, MAX_INPUT);
        const text = try cleanText(allocator, raw);
        if (text.len == 0) return Error.EmptyDocument;
        return .{ .kind = .sheets, .text = text };
    }
    if (eql(ext, "tsv")) {
        const raw = try readFile(allocator, path, MAX_INPUT);
        const text = try tsvToCsv(allocator, raw);
        if (std.mem.trim(u8, text, " \t\r\n").len == 0) return Error.EmptyDocument;
        return .{ .kind = .sheets, .text = text };
    }
    if (eql(ext, "docx")) {
        const text = try docxToMarkdown(allocator, path);
        if (text.len == 0) return Error.EmptyDocument;
        return .{ .kind = .docs, .text = text };
    }
    if (eql(ext, "odt")) {
        const text = try odfTextToMarkdown(allocator, path);
        if (text.len == 0) return Error.EmptyDocument;
        return .{ .kind = .docs, .text = text };
    }
    if (eql(ext, "pdf")) {
        const text = try pdfToMarkdown(allocator, path);
        if (text.len == 0) return Error.EmptyDocument;
        return .{ .kind = .docs, .text = text };
    }
    if (eql(ext, "xlsx")) {
        const text = try xlsxToCsv(allocator, path);
        if (std.mem.trim(u8, text, " \t\r\n").len == 0) return Error.EmptyDocument;
        return .{ .kind = .sheets, .text = text };
    }
    if (eql(ext, "ods")) {
        const text = try odsToCsv(allocator, path);
        if (std.mem.trim(u8, text, " \t\r\n").len == 0) return Error.EmptyDocument;
        return .{ .kind = .sheets, .text = text };
    }
    if (eql(ext, "pptx")) {
        const text = try pptxToJson(allocator, path, display_name);
        return .{ .kind = .presentations, .text = text };
    }
    if (eql(ext, "odp")) {
        const text = try odpToJson(allocator, path, display_name);
        return .{ .kind = .presentations, .text = text };
    }
    return Error.UnsupportedFormat;
}

fn extension(path: []const u8) ?[]const u8 {
    const base = std.fs.path.basename(path);
    const dot = std.mem.lastIndexOfScalar(u8, base, '.') orelse return null;
    if (dot + 1 >= base.len) return null;
    return base[dot + 1 ..];
}

fn eql(a: []const u8, b: []const u8) bool {
    return std.ascii.eqlIgnoreCase(a, b);
}

fn readFile(allocator: std.mem.Allocator, path: []const u8, max_size: usize) ![]u8 {
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    const stat = try file.stat();
    if (stat.size > max_size) return Error.FileTooLarge;
    return file.readToEndAlloc(allocator, max_size);
}

// ---------------------------------------------------------------------------
// ZIP (OOXML / ODF)
// ---------------------------------------------------------------------------

const ZipEntry = struct {
    name: []const u8,
    file_offset: u64,
    compressed_size: u64,
    uncompressed_size: u64,
    crc32: u32,
    method: std.zip.CompressionMethod,
};

fn listZip(allocator: std.mem.Allocator, path: []const u8) ![]ZipEntry {
    const file = std.fs.cwd().openFile(path, .{}) catch return Error.InvalidArchive;
    defer file.close();
    const stream = file.seekableStream();
    var iter = std.zip.Iterator(@TypeOf(stream)).init(stream) catch return Error.InvalidArchive;
    var entries = std.ArrayList(ZipEntry).init(allocator);
    while (iter.next() catch return Error.InvalidArchive) |entry| {
        if (entry.filename_len == 0 or entry.filename_len > 1024) continue;
        const name = try allocator.alloc(u8, entry.filename_len);
        try stream.context.seekTo(entry.header_zip_offset + @sizeOf(std.zip.CentralDirectoryFileHeader));
        const read = try stream.context.reader().readAll(name);
        if (read != entry.filename_len) continue;
        try entries.append(.{
            .name = name,
            .file_offset = entry.file_offset,
            .compressed_size = entry.compressed_size,
            .uncompressed_size = entry.uncompressed_size,
            .crc32 = entry.crc32,
            .method = entry.compression_method,
        });
    }
    return entries.toOwnedSlice();
}

fn readZipEntry(allocator: std.mem.Allocator, path: []const u8, entry: ZipEntry) ![]u8 {
    if (entry.uncompressed_size > MAX_ENTRY) return Error.FileTooLarge;
    const file = std.fs.cwd().openFile(path, .{}) catch return Error.InvalidArchive;
    defer file.close();
    try file.seekTo(entry.file_offset);
    const local = file.reader().readStructEndian(std.zip.LocalFileHeader, .little) catch return Error.InvalidArchive;
    if (!std.mem.eql(u8, &local.signature, &std.zip.local_file_header_sig)) return Error.InvalidArchive;
    const data_offset = entry.file_offset + @sizeOf(std.zip.LocalFileHeader) + local.filename_len + local.extra_len;
    try file.seekTo(data_offset);
    var limited = std.io.limitedReader(file.reader(), entry.compressed_size);
    var out = std.ArrayList(u8).init(allocator);
    const crc = std.zip.decompress(entry.method, entry.uncompressed_size, limited.reader(), out.writer()) catch return Error.InvalidArchive;
    if (crc != entry.crc32) return Error.InvalidArchive;
    return out.toOwnedSlice();
}

fn findZipEntry(entries: []const ZipEntry, name: []const u8) ?ZipEntry {
    for (entries) |entry| if (std.mem.eql(u8, entry.name, name)) return entry;
    return null;
}

fn entryByName(allocator: std.mem.Allocator, path: []const u8, entries: []const ZipEntry, name: []const u8) !?[]u8 {
    const entry = findZipEntry(entries, name) orelse return null;
    return try readZipEntry(allocator, path, entry);
}

fn entryStartingWith(entries: []const ZipEntry, prefix: []const u8, suffix: []const u8) ?ZipEntry {
    for (entries) |entry| {
        if (std.mem.startsWith(u8, entry.name, prefix) and std.mem.endsWith(u8, entry.name, suffix)) return entry;
    }
    return null;
}

// ---------------------------------------------------------------------------
// XML helpers
// ---------------------------------------------------------------------------

const Tag = struct {
    name: []const u8,
    closing: bool,
    self_closing: bool,
    attrs: []const u8,
};

fn parseTag(raw: []const u8) Tag {
    const closing = raw.len > 0 and raw[0] == '/';
    const body = if (closing) raw[1..] else raw;
    const trimmed = std.mem.trimRight(u8, body, " \t\r\n");
    const self_closing = trimmed.len > 0 and trimmed[trimmed.len - 1] == '/';
    const end = std.mem.indexOfAny(u8, trimmed, " \t\r\n/") orelse trimmed.len;
    return .{
        .name = trimmed[0..end],
        .closing = closing,
        .self_closing = self_closing,
        .attrs = trimmed[end..],
    };
}

fn attrValue(tag: Tag, name: []const u8) ?[]const u8 {
    var i: usize = 0;
    while (i < tag.attrs.len) {
        const start = std.mem.indexOfScalarPos(u8, tag.attrs, i, '=') orelse return null;
        const key = std.mem.trim(u8, tag.attrs[i..start], " \t\r\n");
        if (start + 1 >= tag.attrs.len) return null;
        const quote = tag.attrs[start + 1];
        if (quote != '"' and quote != '\'') return null;
        const value_start = start + 2;
        const value_end = std.mem.indexOfScalarPos(u8, tag.attrs, value_start, quote) orelse return null;
        if (std.mem.eql(u8, key, name)) return tag.attrs[value_start..value_end];
        i = value_end + 1;
    }
    return null;
}

fn appendDecoded(out: *std.ArrayList(u8), text: []const u8) !void {
    var i: usize = 0;
    while (i < text.len) {
        if (text[i] == '&') {
            if (std.mem.indexOfScalarPos(u8, text, i, ';')) |semi| {
                if (semi > i + 1 and semi - i <= 12) {
                    const entity = text[i + 1 .. semi];
                    if (std.mem.eql(u8, entity, "amp")) {
                        try out.append('&');
                    } else if (std.mem.eql(u8, entity, "lt")) {
                        try out.append('<');
                    } else if (std.mem.eql(u8, entity, "gt")) {
                        try out.append('>');
                    } else if (std.mem.eql(u8, entity, "quot")) {
                        try out.append('"');
                    } else if (std.mem.eql(u8, entity, "apos")) {
                        try out.append('\'');
                    } else if (entity[0] == '#') {
                        const code = if (entity.len > 1 and (entity[1] == 'x' or entity[1] == 'X'))
                            std.fmt.parseInt(u21, entity[2..], 16) catch 0
                        else
                            std.fmt.parseInt(u21, entity[1..], 10) catch 0;
                        if (code > 0 and code <= 0x10FFFF) {
                            var buf: [4]u8 = undefined;
                            const len = std.unicode.utf8Encode(code, &buf) catch 0;
                            if (len > 0) try out.appendSlice(buf[0..len]);
                        } else {
                            try out.appendSlice(text[i .. semi + 1]);
                        }
                    } else {
                        try out.appendSlice(text[i .. semi + 1]);
                    }
                    i = semi + 1;
                    continue;
                }
            }
        }
        try out.append(text[i]);
        i += 1;
    }
}

const ParagraphOptions = struct {
    para_tags: []const []const u8,
    text_tag: []const u8 = "",
    heading_tags: []const []const u8 = &.{},
};

fn isOneOf(name: []const u8, list: []const []const u8) bool {
    for (list) |item| if (std.mem.eql(u8, name, item)) return true;
    return false;
}

/// Extrae párrafos de un XML de documento. Con `text_tag` vacío se captura el
/// texto directo entre etiquetas (ODF); si no, solo el contenido de esa
/// etiqueta (OOXML: w:t, a:t).
fn xmlParagraphs(allocator: std.mem.Allocator, xml: []const u8, options: ParagraphOptions) ![][]const u8 {
    var list = std.ArrayList([]const u8).init(allocator);
    var buf = std.ArrayList(u8).init(allocator);
    var in_para = false;
    const close_marker = if (options.text_tag.len > 0)
        try std.fmt.allocPrint(allocator, "</{s}>", .{options.text_tag})
    else
        "";

    var i: usize = 0;
    while (i < xml.len) {
        const lt = std.mem.indexOfScalarPos(u8, xml, i, '<') orelse break;
        if (in_para and options.text_tag.len == 0) try appendDecoded(&buf, xml[i..lt]);
        if (std.mem.startsWith(u8, xml[lt..], "<!--")) {
            i = (std.mem.indexOfPos(u8, xml, lt + 4, "-->") orelse break) + 3;
            continue;
        }
        if (std.mem.startsWith(u8, xml[lt..], "<![CDATA[")) {
            const end = std.mem.indexOfPos(u8, xml, lt + 9, "]]>") orelse break;
            if (in_para) try buf.appendSlice(xml[lt + 9 .. end]);
            i = end + 3;
            continue;
        }
        const gt = std.mem.indexOfScalarPos(u8, xml, lt, '>') orelse break;
        const tag = parseTag(xml[lt + 1 .. gt]);
        i = gt + 1;
        if (tag.name.len == 0) continue;

        if (tag.closing) {
            if (isOneOf(tag.name, options.para_tags) and in_para) {
                try pushParagraph(allocator, &list, buf.items);
                buf.clearRetainingCapacity();
                in_para = false;
            }
            continue;
        }
        if (isOneOf(tag.name, options.para_tags)) {
            if (tag.self_closing) continue;
            if (in_para) {
                try pushParagraph(allocator, &list, buf.items);
                buf.clearRetainingCapacity();
            }
            in_para = true;
            if (isOneOf(tag.name, options.heading_tags)) try buf.appendSlice("## ");
            continue;
        }
        if (tag.self_closing) {
            if (std.mem.eql(u8, tag.name, "text:s")) {
                if (in_para) try buf.append(' ');
            } else if (std.mem.eql(u8, tag.name, "text:tab") or std.mem.eql(u8, tag.name, "w:tab") or std.mem.eql(u8, tag.name, "w:ptab")) {
                if (in_para) try buf.append('\t');
            } else if (std.mem.eql(u8, tag.name, "text:line-break") or std.mem.eql(u8, tag.name, "w:br") or std.mem.eql(u8, tag.name, "w:cr") or std.mem.eql(u8, tag.name, "a:br")) {
                if (in_para) try buf.append('\n');
            }
            continue;
        }
        if (options.text_tag.len > 0 and std.mem.eql(u8, tag.name, options.text_tag)) {
            const close = std.mem.indexOfPos(u8, xml, i, close_marker) orelse break;
            if (in_para) try appendDecoded(&buf, xml[i..close]);
            i = @min(close + close_marker.len, xml.len);
        }
    }
    if (in_para) try pushParagraph(allocator, &list, buf.items);
    return list.toOwnedSlice();
}

fn pushParagraph(allocator: std.mem.Allocator, list: *std.ArrayList([]const u8), raw: []const u8) !void {
    const trimmed = std.mem.trim(u8, raw, " \t\r\n");
    if (trimmed.len == 0) return;
    try list.append(try allocator.dupe(u8, trimmed));
}

// ---------------------------------------------------------------------------
// DOCX / ODT / PPTX / ODP
// ---------------------------------------------------------------------------

fn docxToMarkdown(allocator: std.mem.Allocator, path: []const u8) ![]const u8 {
    const entries = try listZip(allocator, path);
    const xml = (try entryByName(allocator, path, entries, "word/document.xml")) orelse return Error.InvalidArchive;
    const paragraphs = try xmlParagraphs(allocator, xml, .{ .para_tags = &.{"w:p"}, .text_tag = "w:t" });
    return joinParagraphs(allocator, paragraphs);
}

fn odfTextToMarkdown(allocator: std.mem.Allocator, path: []const u8) ![]const u8 {
    const entries = try listZip(allocator, path);
    const xml = (try entryByName(allocator, path, entries, "content.xml")) orelse return Error.InvalidArchive;
    const paragraphs = try xmlParagraphs(allocator, xml, .{
        .para_tags = &.{ "text:p", "text:h" },
        .heading_tags = &.{"text:h"},
    });
    return joinParagraphs(allocator, paragraphs);
}

fn joinParagraphs(allocator: std.mem.Allocator, paragraphs: []const []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    for (paragraphs, 0..) |paragraph, index| {
        if (index > 0) try out.appendSlice("\n\n");
        try out.appendSlice(paragraph);
    }
    try out.append('\n');
    return out.toOwnedSlice();
}

fn slideSortLess(_: void, a: ZipEntry, b: ZipEntry) bool {
    return slideNumber(a.name) < slideNumber(b.name);
}

fn slideNumber(name: []const u8) usize {
    const base = std.fs.path.basename(name);
    const stem = if (std.mem.lastIndexOfScalar(u8, base, '.')) |dot| base[0..dot] else base;
    var digits: usize = 0;
    var i = stem.len;
    while (i > 0) : (i -= 1) {
        if (std.ascii.isDigit(stem[i - 1])) {
            digits += 1;
        } else break;
    }
    if (digits == 0) return 0;
    return std.fmt.parseInt(usize, stem[stem.len - digits ..], 10) catch 0;
}

fn pptxToJson(allocator: std.mem.Allocator, path: []const u8, display_name: []const u8) ![]const u8 {
    const entries = try listZip(allocator, path);
    var slides = std.ArrayList(ZipEntry).init(allocator);
    for (entries) |entry| {
        if (std.mem.startsWith(u8, entry.name, "ppt/slides/slide") and std.mem.endsWith(u8, entry.name, ".xml")) {
            try slides.append(entry);
        }
    }
    if (slides.items.len == 0) return Error.InvalidArchive;
    std.mem.sort(ZipEntry, slides.items, {}, slideSortLess);

    var deck = std.ArrayList(SlideOut).init(allocator);
    for (slides.items) |entry| {
        const xml = try readZipEntry(allocator, path, entry);
        const paragraphs = try xmlParagraphs(allocator, xml, .{ .para_tags = &.{"a:p"}, .text_tag = "a:t" });
        try deck.append(slideFromParagraphs(allocator, paragraphs));
    }
    return stringifyDeck(allocator, display_name, deck.items);
}

fn odpToJson(allocator: std.mem.Allocator, path: []const u8, display_name: []const u8) ![]const u8 {
    const entries = try listZip(allocator, path);
    const xml = (try entryByName(allocator, path, entries, "content.xml")) orelse return Error.InvalidArchive;
    var deck = std.ArrayList(SlideOut).init(allocator);

    const open = "<draw:page";
    const close = "</draw:page>";
    var index: usize = 0;
    var found = false;
    while (std.mem.indexOfPos(u8, xml, index, open)) |start| {
        const gt = std.mem.indexOfScalarPos(u8, xml, start, '>') orelse break;
        const tag = parseTag(xml[start + 1 .. gt]);
        index = gt + 1;
        if (tag.self_closing) continue;
        const end = std.mem.indexOfPos(u8, xml, index, close) orelse break;
        const page_xml = xml[index..end];
        index = end + close.len;
        const paragraphs = try xmlParagraphs(allocator, page_xml, .{
            .para_tags = &.{ "text:p", "text:h" },
            .heading_tags = &.{"text:h"},
        });
        if (paragraphs.len == 0) continue;
        found = true;
        try deck.append(slideFromParagraphs(allocator, paragraphs));
    }
    if (!found) {
        const paragraphs = try xmlParagraphs(allocator, xml, .{
            .para_tags = &.{ "text:p", "text:h" },
            .heading_tags = &.{"text:h"},
        });
        if (paragraphs.len == 0) return Error.EmptyDocument;
        try deck.append(slideFromParagraphs(allocator, paragraphs));
    }
    return stringifyDeck(allocator, display_name, deck.items);
}

const SlideOut = struct {
    title: []const u8,
    content: []const u8,
};

const DeckOut = struct {
    title: []const u8,
    slides: []const SlideOut,
};

fn slideFromParagraphs(allocator: std.mem.Allocator, paragraphs: []const []const u8) SlideOut {
    if (paragraphs.len == 0) return .{ .title = "Diapositiva", .content = "" };
    var content = std.ArrayList(u8).init(allocator);
    for (paragraphs[1..], 0..) |paragraph, index| {
        if (index > 0) content.appendSlice("\n\n") catch {};
        content.appendSlice(paragraph) catch {};
    }
    return .{
        .title = paragraphs[0],
        .content = content.items,
    };
}

fn stringifyDeck(allocator: std.mem.Allocator, display_name: []const u8, slides: []const SlideOut) ![]const u8 {
    const deck = DeckOut{ .title = display_name, .slides = slides };
    return std.json.stringifyAlloc(allocator, deck, .{});
}

// ---------------------------------------------------------------------------
// XLSX / ODS
// ---------------------------------------------------------------------------

fn xlsxToCsv(allocator: std.mem.Allocator, path: []const u8) ![]const u8 {
    const entries = try listZip(allocator, path);
    var shared = std.ArrayList([]const u8).init(allocator);
    if (try entryByName(allocator, path, entries, "xl/sharedStrings.xml")) |xml| {
        try parseSharedStrings(allocator, xml, &shared);
    }
    const sheet_entry = findZipEntry(entries, "xl/worksheets/sheet1.xml") orelse
        entryStartingWith(entries, "xl/worksheets/", ".xml") orelse return Error.InvalidArchive;
    const sheet_xml = try readZipEntry(allocator, path, sheet_entry);
    return parseSheetCsv(allocator, sheet_xml, shared.items);
}

fn parseSharedStrings(allocator: std.mem.Allocator, xml: []const u8, out: *std.ArrayList([]const u8)) !void {
    var i: usize = 0;
    while (std.mem.indexOfPos(u8, xml, i, "<si")) |start| {
        const gt = std.mem.indexOfScalarPos(u8, xml, start, '>') orelse break;
        const tag = parseTag(xml[start + 1 .. gt]);
        i = gt + 1;
        if (tag.self_closing) {
            try out.append("");
            continue;
        }
        const end = std.mem.indexOfPos(u8, xml, i, "</si>") orelse break;
        const inner = xml[i..end];
        i = end + 5;
        var text = std.ArrayList(u8).init(allocator);
        var j: usize = 0;
        while (std.mem.indexOfPos(u8, inner, j, "<t")) |tstart| {
            const tgt = std.mem.indexOfScalarPos(u8, inner, tstart, '>') orelse break;
            const ttag = parseTag(inner[tstart + 1 .. tgt]);
            j = tgt + 1;
            if (ttag.self_closing) continue;
            const tend = std.mem.indexOfPos(u8, inner, j, "</t>") orelse break;
            try appendDecoded(&text, inner[j..tend]);
            j = tend + 4;
        }
        try out.append(try text.toOwnedSlice());
    }
}

fn parseSheetCsv(allocator: std.mem.Allocator, xml: []const u8, shared: []const []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    var i: usize = 0;
    var rows: usize = 0;
    while (std.mem.indexOfPos(u8, xml, i, "<row")) |row_start| {
        if (rows >= MAX_SHEET_ROWS) break;
        const row_gt = std.mem.indexOfScalarPos(u8, xml, row_start, '>') orelse break;
        const row_tag = parseTag(xml[row_start + 1 .. row_gt]);
        i = row_gt + 1;
        if (!std.mem.eql(u8, row_tag.name, "row")) continue;
        if (row_tag.self_closing) {
            try out.append('\n');
            rows += 1;
            continue;
        }
        const row_end = std.mem.indexOfPos(u8, xml, i, "</row>") orelse break;
        const row_xml = xml[i..row_end];
        i = row_end + 6;
        try appendSheetRow(allocator, &out, row_xml, shared);
        rows += 1;
    }
    return out.toOwnedSlice();
}

fn appendSheetRow(allocator: std.mem.Allocator, out: *std.ArrayList(u8), row_xml: []const u8, shared: []const []const u8) !void {
    var cells = std.ArrayList([]const u8).init(allocator);
    var max_column: usize = 0;
    var i: usize = 0;
    while (std.mem.indexOfPos(u8, row_xml, i, "<c")) |cell_start| {
        const gt = std.mem.indexOfScalarPos(u8, row_xml, cell_start, '>') orelse break;
        const tag = parseTag(row_xml[cell_start + 1 .. gt]);
        i = gt + 1;
        if (!std.mem.eql(u8, tag.name, "c")) continue;
        const reference = attrValue(tag, "r");
        const column = if (reference) |ref| columnIndex(ref) else max_column;
        if (column >= MAX_SHEET_COLUMNS) continue;
        var value: []const u8 = "";
        if (!tag.self_closing) {
            const cell_end = std.mem.indexOfPos(u8, row_xml, i, "</c>") orelse break;
            const inner = row_xml[i..cell_end];
            i = cell_end + 4;
            value = try cellValue(allocator, inner, attrValue(tag, "t"), shared);
        }
        while (cells.items.len <= column) try cells.append("");
        cells.items[column] = value;
        if (column + 1 > max_column) max_column = column + 1;
    }
    for (cells.items, 0..) |cell, index| {
        if (index > 0) try out.append(',');
        try appendCsvField(out, cell);
    }
    try out.append('\n');
}

fn cellValue(allocator: std.mem.Allocator, inner: []const u8, cell_type: ?[]const u8, shared: []const []const u8) ![]const u8 {
    const kind = cell_type orelse "";
    if (std.mem.eql(u8, kind, "inlineStr")) {
        var text = std.ArrayList(u8).init(allocator);
        var j: usize = 0;
        while (std.mem.indexOfPos(u8, inner, j, "<t")) |tstart| {
            const tgt = std.mem.indexOfScalarPos(u8, inner, tstart, '>') orelse break;
            const ttag = parseTag(inner[tstart + 1 .. tgt]);
            j = tgt + 1;
            if (ttag.self_closing) continue;
            const tend = std.mem.indexOfPos(u8, inner, j, "</t>") orelse break;
            try appendDecoded(&text, inner[j..tend]);
            j = tend + 4;
        }
        return text.toOwnedSlice();
    }
    const value = valueTag(inner) orelse return "";
    if (std.mem.eql(u8, kind, "s")) {
        const index = std.fmt.parseInt(usize, std.mem.trim(u8, value, " \t\r\n"), 10) catch return "";
        return if (index < shared.len) shared[index] else "";
    }
    if (std.mem.eql(u8, kind, "b")) return if (std.mem.eql(u8, std.mem.trim(u8, value, " \t\r\n"), "1")) "VERDADERO" else "FALSO";
    if (std.mem.eql(u8, kind, "str")) return value;
    return value;
}

fn valueTag(inner: []const u8) ?[]const u8 {
    const start = std.mem.indexOf(u8, inner, "<v>") orelse
        std.mem.indexOf(u8, inner, "<v ") orelse return null;
    const gt = std.mem.indexOfScalarPos(u8, inner, start, '>') orelse return null;
    const end = std.mem.indexOfPos(u8, inner, gt + 1, "</v>") orelse return null;
    return inner[gt + 1 .. end];
}

fn columnIndex(reference: []const u8) usize {
    var value: usize = 0;
    for (reference) |char| {
        if (char >= 'A' and char <= 'Z') {
            value = value * 26 + (char - 'A' + 1);
        } else if (char >= 'a' and char <= 'z') {
            value = value * 26 + (char - 'a' + 1);
        } else break;
    }
    return if (value == 0) 0 else value - 1;
}

fn appendCsvField(out: *std.ArrayList(u8), value: []const u8) !void {
    const needs_quotes = std.mem.indexOfAny(u8, value, ",\"\r\n") != null;
    if (!needs_quotes) {
        try out.appendSlice(value);
        return;
    }
    try out.append('"');
    for (value) |char| {
        if (char == '"') try out.append('"');
        try out.append(char);
    }
    try out.append('"');
}

fn odsToCsv(allocator: std.mem.Allocator, path: []const u8) ![]const u8 {
    const entries = try listZip(allocator, path);
    const xml = (try entryByName(allocator, path, entries, "content.xml")) orelse return Error.InvalidArchive;
    var out = std.ArrayList(u8).init(allocator);
    var i: usize = 0;
    var rows: usize = 0;
    while (std.mem.indexOfPos(u8, xml, i, "<table:table-row")) |row_start| {
        if (rows >= MAX_SHEET_ROWS) break;
        const row_gt = std.mem.indexOfScalarPos(u8, xml, row_start, '>') orelse break;
        const row_tag = parseTag(xml[row_start + 1 .. row_gt]);
        i = row_gt + 1;
        if (row_tag.self_closing) continue;
        const row_end = std.mem.indexOfPos(u8, xml, i, "</table:table-row>") orelse break;
        const row_xml = xml[i..row_end];
        i = row_end + "</table:table-row>".len;
        try appendOdsRow(allocator, &out, row_xml);
        rows += 1;
    }
    return out.toOwnedSlice();
}

fn appendOdsRow(allocator: std.mem.Allocator, out: *std.ArrayList(u8), row_xml: []const u8) !void {
    var first = true;
    var i: usize = 0;
    while (std.mem.indexOfPos(u8, row_xml, i, "<table:table-cell")) |cell_start| {
        const gt = std.mem.indexOfScalarPos(u8, row_xml, cell_start, '>') orelse break;
        const tag = parseTag(row_xml[cell_start + 1 .. gt]);
        i = gt + 1;
        if (tag.self_closing) {
            if (!first) try out.append(',');
            first = false;
            continue;
        }
        const cell_end = std.mem.indexOfPos(u8, row_xml, i, "</table:table-cell>") orelse break;
        const inner = row_xml[i..cell_end];
        i = cell_end + "</table:table-cell>".len;
        const repeated = if (attrValue(tag, "table:number-columns-repeated")) |raw|
            std.fmt.parseInt(usize, raw, 10) catch 1
        else
            1;
        const paragraphs = try xmlParagraphs(allocator, inner, .{ .para_tags = &.{"text:p"} });
        var text = std.ArrayList(u8).init(allocator);
        for (paragraphs, 0..) |paragraph, index| {
            if (index > 0) try text.append(' ');
            try text.appendSlice(paragraph);
        }
        var copy: usize = 0;
        while (copy < @min(repeated, 50)) : (copy += 1) {
            if (!first) try out.append(',');
            first = false;
            try appendCsvField(out, text.items);
        }
    }
    try out.append('\n');
}

// ---------------------------------------------------------------------------
// PDF (pdftotext)
// ---------------------------------------------------------------------------

fn pdfToMarkdown(allocator: std.mem.Allocator, path: []const u8) ![]const u8 {
    const result = std.process.Child.run(.{
        .allocator = allocator,
        .argv = &.{ "pdftotext", "-layout", "-enc", "UTF-8", path, "-" },
        .max_output_bytes = 16 * 1024 * 1024,
    }) catch return Error.PdfToolMissing;
    const ok = switch (result.term) {
        .Exited => |code| code == 0,
        else => false,
    };
    if (!ok) return Error.PdfFailed;
    return cleanText(allocator, result.stdout);
}

/// Convierte un PDF en una presentación: cada página es una diapositiva.
pub fn pdfToSlides(allocator: std.mem.Allocator, path: []const u8, display_name: []const u8) ![]const u8 {
    const result = std.process.Child.run(.{
        .allocator = allocator,
        .argv = &.{ "pdftotext", "-layout", "-enc", "UTF-8", path, "-" },
        .max_output_bytes = 16 * 1024 * 1024,
    }) catch return Error.PdfToolMissing;
    const ok = switch (result.term) {
        .Exited => |code| code == 0,
        else => false,
    };
    if (!ok) return Error.PdfFailed;

    var deck = std.ArrayList(SlideOut).init(allocator);
    var pages = std.mem.splitScalar(u8, result.stdout, 0x0c);
    while (pages.next()) |page| {
        const text = try cleanText(allocator, page);
        if (text.len == 0) continue;
        var lines = std.mem.splitScalar(u8, text, '\n');
        var title: []const u8 = "";
        var body = std.ArrayList(u8).init(allocator);
        while (lines.next()) |line| {
            const trimmed = std.mem.trim(u8, line, " \t\r");
            if (trimmed.len == 0) continue;
            if (title.len == 0) {
                title = trimmed;
                continue;
            }
            if (body.items.len > 0) try body.appendSlice("\n\n");
            try body.appendSlice(trimmed);
        }
        if (title.len == 0) continue;
        try deck.append(.{ .title = title, .content = body.items });
    }
    if (deck.items.len == 0) return Error.EmptyDocument;
    return stringifyDeck(allocator, display_name, deck.items);
}

// ---------------------------------------------------------------------------
// Texto
// ---------------------------------------------------------------------------

fn cleanText(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    var normalized = std.ArrayList(u8).init(allocator);
    var i: usize = 0;
    while (i < raw.len) : (i += 1) {
        const char = raw[i];
        if (char == 0 or char == 0x0c) continue;
        if (char == '\r') {
            if (i + 1 < raw.len and raw[i + 1] == '\n') continue;
            try normalized.append('\n');
            continue;
        }
        try normalized.append(char);
    }
    var out = std.ArrayList(u8).init(allocator);
    var newline_run: usize = 0;
    var line = std.ArrayList(u8).init(allocator);
    var j: usize = 0;
    while (j <= normalized.items.len) : (j += 1) {
        const at_end = j == normalized.items.len;
        const char = if (at_end) '\n' else normalized.items[j];
        if (!at_end and char != '\n') {
            try line.append(char);
            continue;
        }
        const trimmed = std.mem.trimRight(u8, line.items, " \t");
        if (trimmed.len == 0) {
            newline_run += 1;
        } else {
            while (newline_run > 0 and out.items.len > 0) : (newline_run -= 1) {
                if (out.items[out.items.len - 1] != '\n') try out.append('\n');
                if (newline_run > 1) try out.append('\n');
            }
            newline_run = 0;
            try out.appendSlice(trimmed);
            try out.append('\n');
        }
        line.clearRetainingCapacity();
    }
    const text = std.mem.trim(u8, out.items, "\n");
    if (text.len == 0) return "";
    const result = try allocator.alloc(u8, text.len + 1);
    @memcpy(result[0..text.len], text);
    result[text.len] = '\n';
    return result;
}

fn tsvToCsv(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    var line = std.ArrayList(u8).init(allocator);
    var i: usize = 0;
    while (i <= raw.len) : (i += 1) {
        const at_end = i == raw.len;
        const char = if (at_end) '\n' else raw[i];
        if (char == '\r') continue;
        if (char == '\t') {
            try appendCsvField(&out, line.items);
            try out.append(',');
            line.clearRetainingCapacity();
            continue;
        }
        if (char == '\n') {
            try appendCsvField(&out, line.items);
            try out.append('\n');
            line.clearRetainingCapacity();
            continue;
        }
        try line.append(char);
    }
    return out.toOwnedSlice();
}

// ---------------------------------------------------------------------------
// Pruebas
// ---------------------------------------------------------------------------

test "kindFor reconoce documentos y rechaza imágenes y código" {
    try std.testing.expectEqual(Kind.docs, kindFor("/x/nota.MD").?);
    try std.testing.expectEqual(Kind.docs, kindFor("/x/taller.docx").?);
    try std.testing.expectEqual(Kind.docs, kindFor("/x/guia.pdf").?);
    try std.testing.expectEqual(Kind.sheets, kindFor("/x/tabla.xlsx").?);
    try std.testing.expectEqual(Kind.presentations, kindFor("/x/charla.pptx").?);
    try std.testing.expectEqual(Kind.html, kindFor("/x/pagina.html").?);
    try std.testing.expect(kindFor("/x/foto.jpeg") == null);
    try std.testing.expect(kindFor("/x/script.sh") == null);
    try std.testing.expect(kindFor("/x/sin-extension") == null);
}

test "docx XML extrae párrafos, tabuladores y entidades" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const xml =
        \\<w:document><w:body>
        \\<w:p><w:r><w:t>Hola &amp; bienvenido</w:t></w:r></w:p>
        \\<w:p><w:r><w:t>Segunda</w:t></w:r><w:r><w:tab/><w:t>línea</w:t></w:r></w:p>
        \\<w:p/>
        \\<w:p><w:r><w:t>Fin</w:t></w:r><w:r><w:br/><w:t>salto</w:t></w:r></w:p>
        \\</w:body></w:document>
    ;
    const paragraphs = try xmlParagraphs(allocator, xml, .{ .para_tags = &.{"w:p"}, .text_tag = "w:t" });
    try std.testing.expectEqual(@as(usize, 3), paragraphs.len);
    try std.testing.expectEqualStrings("Hola & bienvenido", paragraphs[0]);
    try std.testing.expectEqualStrings("Segunda\tlínea", paragraphs[1]);
    try std.testing.expectEqualStrings("Fin\nsalto", paragraphs[2]);
}

test "pptx XML produce párrafos en orden" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const xml =
        \\<p:sld><p:cSld><p:spTree><p:sp><p:txBody>
        \\<a:p><a:r><a:t>Título de la diapositiva</a:t></a:r></a:p>
        \\<a:p><a:r><a:t>Primer punto</a:t></a:r></a:p>
        \\<a:p><a:r><a:t>Segundo punto</a:t></a:r></a:p>
        \\</p:txBody></p:sp></p:spTree></p:cSld></p:sld>
    ;
    const paragraphs = try xmlParagraphs(allocator, xml, .{ .para_tags = &.{"a:p"}, .text_tag = "a:t" });
    try std.testing.expectEqual(@as(usize, 3), paragraphs.len);
    const slide = slideFromParagraphs(allocator, paragraphs);
    try std.testing.expectEqualStrings("Título de la diapositiva", slide.title);
    try std.testing.expectEqualStrings("Primer punto\n\nSegundo punto", slide.content);
}

test "xlsx sharedStrings y hoja se convierten a CSV con comillas" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const shared_xml =
        \\<sst><si><t>Nombre</t></si><si><t>Nota, final</t></si><si><t>Comillas "x"</t></si></sst>
    ;
    var shared = std.ArrayList([]const u8).init(allocator);
    try parseSharedStrings(allocator, shared_xml, &shared);
    try std.testing.expectEqual(@as(usize, 3), shared.items.len);
    try std.testing.expectEqualStrings("Nota, final", shared.items[1]);

    const sheet_xml =
        \\<worksheet><sheetData>
        \\<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>Puntos</t></is></c></row>
        \\<row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>8.5</v></c></row>
        \\<row r="3"><c r="A3" t="s"><v>2</v></c><c r="C3"><v>3</v></c></row>
        \\</sheetData></worksheet>
    ;
    const csv = try parseSheetCsv(allocator, sheet_xml, shared.items);
    try std.testing.expectEqualStrings("Nombre,Puntos\n\"Nota, final\",8.5\n\"Comillas \"\"x\"\"\",,3\n", csv);
}

test "cleanText normaliza saltos y colapsa líneas en blanco" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const cleaned = try cleanText(allocator, "Hola\r\n\r\n\r\nMundo  \r\n");
    try std.testing.expectEqualStrings("Hola\n\nMundo\n", cleaned);
    const empty = try cleanText(allocator, " \r\n\t\n");
    try std.testing.expectEqualStrings("", empty);
}

test "ODF párrafos y encabezados" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const xml =
        \\<office:text><text:h text:outline-level="1">Título</text:h>
        \\<text:p>Texto con <text:span>énfasis</text:span> y espacio<text:s/>final.</text:p>
        \\</office:text>
    ;
    const paragraphs = try xmlParagraphs(allocator, xml, .{
        .para_tags = &.{ "text:p", "text:h" },
        .heading_tags = &.{"text:h"},
    });
    try std.testing.expectEqual(@as(usize, 2), paragraphs.len);
    try std.testing.expectEqualStrings("## Título", paragraphs[0]);
    try std.testing.expectEqualStrings("Texto con énfasis y espacio final.", paragraphs[1]);
}
