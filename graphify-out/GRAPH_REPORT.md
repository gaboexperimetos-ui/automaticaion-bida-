# Graph Report - styp  (2026-11-27)

## Corpus Check
- 63 files · ~205,500 words
- Verdict: corpus is large enough that graph structure adds value.
- Unclassified: 20 file(s) not represented in the graph (top: .woff2 9, .css 7, (none) 2)

## Summary
- 1488 nodes · 3245 edges · 72 communities (64 shown, 8 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 42 edges (avg confidence: 0.85)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `7b00055d`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- App
- notes.zig
- importer.zig
- mcp.zig
- sqlite.zig
- office.zig
- calendar_view.zig
- panel.zig
- calendar-ics.js
- ai.zig
- cyberhex.zig
- src/main.zig
- app.js
- http_smoke.py
- tasks.zig
- flashcards.zig
- oauth_google.zig
- schedule_view.zig
- mcp_client.zig
- classroom.zig
- boot
- steps
- zig-validation.yml
- initCalendar
- notes
- academico.zig
- desktop/main.zig
- libraryRoute
- shell.js
- google_workspace.zig
- initNotesEditor
- sheetRoute
- calGrid
- audio.zig
- config.zig
- jsonutil.zig
- officeDir
- busqueda.zig
- storeConverted
- submenuWeight
- subjectDetailHtml
- esc
- connections
- charts.js
- el
- tests.zig
- charts.zig
- nativesync.zig
- std
- chartEmpty
- portal
- OfficeError
- fetchSubjectDetail
- renderMarkup
- ImportStatus
- kpi
- syncThemeUI
- initModelPicker
- calendar_annotations
- audio.js
- createUnique
- 20240101000011_flashcards.sql
- ConnState
- 20240101000003_ai.sql
- 20240101000005_usage_metrics.sql
- TaskScope
- install.sh
- 20240101000012_mikareno_cache.sql
- security.zig
- presentationRoute

## God Nodes (most connected - your core abstractions)
1. `App` - 58 edges
2. `callTool()` - 39 edges
3. `initNotesEditor()` - 26 edges
4. `initCalendar()` - 23 edges
5. `esc()` - 23 edges
6. `boot()` - 20 edges
7. `msgErr()` - 20 edges
8. `sheetRoute()` - 19 edges
9. `libraryRoute()` - 18 edges
10. `presentationRoute()` - 18 edges

## Surprising Connections (you probably didn't know these)
- `notes_search_fts_ad` --triggers--> `notes`  [EXTRACTED]
  migrations/20240101000006_notes_search_fts.sql → migrations/20240101000000_init.sql
- `notes_search_fts_ai` --triggers--> `notes`  [EXTRACTED]
  migrations/20240101000006_notes_search_fts.sql → migrations/20240101000000_init.sql
- `notes_search_fts_au` --triggers--> `notes`  [EXTRACTED]
  migrations/20240101000006_notes_search_fts.sql → migrations/20240101000000_init.sql
- `idx_users_oauth` --indexes--> `users`  [EXTRACTED]
  migrations/20240101000009_oauth_google.sql → migrations/20240101000000_init.sql
- `idx_users_oauth_email` --indexes--> `users`  [EXTRACTED]
  migrations/20240101000009_oauth_google.sql → migrations/20240101000000_init.sql

## Import Cycles
- None detected.

## Communities (72 total, 8 thin omitted)

### Community 0 - "App"
Cohesion: 0.06
Nodes (32): academicTool(), AcademicView, asistencia, evaluaciones, horario, mensajes, perfil, tareas (+24 more)

### Community 1 - "notes.zig"
Cohesion: 0.08
Nodes (78): annotationEnvelope(), annotations(), apiNoteDispatch(), apiNoteRoute(), apiNotes(), appendFtsCards(), appendFtsResults(), appendIcsLine() (+70 more)

### Community 2 - "importer.zig"
Cohesion: 0.08
Nodes (57): appendCsvField(), appendDecoded(), appendOdsRow(), appendSheetRow(), attrValue(), cellValue(), cleanText(), columnIndex() (+49 more)

### Community 3 - "mcp.zig"
Cohesion: 0.08
Nodes (53): aiCost(), appStatus(), argI64(), backupDir(), backupNow(), backups(), calendarAdd(), calendarDelete() (+45 more)

### Community 4 - "sqlite.zig"
Cohesion: 0.07
Nodes (29): Db, Error, OutOfMemory, SqliteError, sqlite3_backup_finish(), sqlite3_backup_init(), sqlite3_backup_step(), sqlite3_bind_double() (+21 more)

### Community 5 - "office.zig"
Cohesion: 0.08
Nodes (40): appendCalendarSection(), appendGradeSection(), appendIdentitySection(), appendNotesSection(), appendTasksSection(), buildStudentReport(), CellRef, collectReportGradeRows() (+32 more)

### Community 6 - "calendar_view.zig"
Cohesion: 0.10
Nodes (45): academicEventsIcs(), annotationOn(), annotationTime(), Anotacion, appendChips(), appendDay(), appendHeader(), appendIcsLine() (+37 more)

### Community 7 - "panel.zig"
Cohesion: 0.06
Nodes (36): applyCached(), applyFull(), Axis, CachedFull, chartHeatmap(), CivilDate, Clase, collectPartialIds() (+28 more)

### Community 8 - "calendar-ics.js"
Cohesion: 0.08
Nodes (35): RFC-5545, addDays(), dateString(), identityKeys(), localParts(), pad(), parseDateTime(), parseEvent() (+27 more)

### Community 9 - "ai.zig"
Cohesion: 0.19
Nodes (26): appendFrame(), budgetExhausted(), callProvider(), chat(), ChatMessage, conversationMessages(), conversations(), cost() (+18 more)

### Community 10 - "cyberhex.zig"
Cohesion: 0.07
Nodes (32): Accion, next, pause, play, previous, seek, toggle, call() (+24 more)

### Community 11 - "src/main.zig"
Cohesion: 0.24
Nodes (7): BindAddress, builtinVersion(), main(), parseAddress(), peerAddress(), printHelp(), serve()

### Community 12 - "app.js"
Cohesion: 0.13
Nodes (32): addChatMessage(), applySettingsVisuals(), bindDropdown(), closeDropdowns(), closeModal(), createOffice(), esc(), fieldHTML() (+24 more)

### Community 13 - "http_smoke.py"
Cohesion: 0.13
Nodes (11): json_body(), main(), MockMcpHandler, NoRedirect, request(), run_auth_smoke(), run_external_integrations_smoke(), run_mainline_merge_smoke() (+3 more)

### Community 14 - "tasks.zig"
Cohesion: 0.09
Nodes (26): academicText(), collect(), count(), Counts, esc(), fetchNotes(), filterLink(), fromClassroom() (+18 more)

### Community 15 - "flashcards.zig"
Cohesion: 0.08
Nodes (39): Card, cardFrom(), cardValue(), Civil, civilFromDays(), createCard(), daysFromCivil(), getCard() (+31 more)

### Community 16 - "oauth_google.zig"
Cohesion: 0.12
Nodes (25): authorizationUrl(), buildStateCookie(), calendarEvents(), clearStateCookie(), Config, Error, BadResponse, EmailMissing (+17 more)

### Community 17 - "schedule_view.zig"
Cohesion: 0.13
Nodes (24): agendaItems(), buildRows(), Clase, clasesDe(), classAt(), datePart(), dayHasClasses(), Dia (+16 more)

### Community 18 - "mcp_client.zig"
Cohesion: 0.06
Nodes (38): Asset, assetByPath(), compressGzip(), find(), gzipForPath(), indexOf(), warmCompression(), CachedProbe (+30 more)

### Community 19 - "classroom.zig"
Cohesion: 0.14
Nodes (22): accessTokenFor(), Announcement, Course, CoursesResult, Detail, detailHtml(), dueLabel(), fetchAnnouncements() (+14 more)

### Community 20 - "boot"
Cohesion: 0.14
Nodes (24): aiCatchPrompt(), boot(), importObsidian(), initAiLegacy(), initAssistant(), buildPrompt(), ctx(), initChatWorkspace() (+16 more)

### Community 21 - "steps"
Cohesion: 0.08
Nodes (23): name, runs-on, steps, concurrency, cancel-in-progress, group, GH_TOKEN, jobs (+15 more)

### Community 22 - "zig-validation.yml"
Cohesion: 0.08
Nodes (23): concurrency, cancel-in-progress, group, jobs, validate, name, on, pull_request (+15 more)

### Community 23 - "initCalendar"
Cohesion: 0.22
Nodes (22): initCalendar(), dayPanel(), deleteEntry(), describeIcsEvent(), editEntry(), ensureCalendarIcsInput(), gridHost(), importCalendarIcs() (+14 more)

### Community 24 - "notes"
Cohesion: 0.12
Nodes (16): app_config, notes, sessions, users, idx_note_versions_note_id, note_versions, notes_fts, idx_sessions_expires_at (+8 more)

### Community 25 - "academico.zig"
Cohesion: 0.23
Nodes (17): asistenciaHtml(), esc(), examenesHtml(), fecha(), fila(), horarioHtml(), kpi(), mensajeItem() (+9 more)

### Community 26 - "desktop/main.zig"
Cohesion: 0.17
Nodes (16): applyWindowCss(), buildWindow(), configDir(), configPath(), GdkEventKey, injectGlassCss(), loadGeometry(), main() (+8 more)

### Community 27 - "libraryRoute"
Cohesion: 0.31
Nodes (17): appendSlugCodepoint(), documents(), fileExists(), formField(), handle(), htmlPages(), hxRedirect(), library() (+9 more)

### Community 28 - "shell.js"
Cohesion: 0.21
Nodes (16): done(), initCollections(), initCollectionsSafe(), initMusicPlayer(), musicAddUrl(), musicAll(), musicCurrent(), musicPlay() (+8 more)

### Community 29 - "google_workspace.zig"
Cohesion: 0.16
Nodes (17): accessTokenFor(), Contact, contacts(), ContactsResult, docKindLabel(), docSectionHtml(), DocText, downloadFile() (+9 more)

### Community 30 - "initNotesEditor"
Cohesion: 0.21
Nodes (19): initNotesEditor(), buildSaveButton(), debouncedRender(), filterLibrary(), hydrate(), insert(), linePrefix(), markDirty() (+11 more)

### Community 31 - "sheetRoute"
Cohesion: 0.22
Nodes (20): documentRoute(), download(), editPresentation(), errorPage(), htmlPageRoute(), humanSizeFromPath(), importResultPage(), libraryPage() (+12 more)

### Community 32 - "calGrid"
Cohesion: 0.14
Nodes (17): calGrid(), capitalize(), chartLoad14(), chartWeek(), civilFromDays(), classesList(), daysFromCivil(), diasDelMes() (+9 more)

### Community 33 - "audio.zig"
Cohesion: 0.14
Nodes (13): buildMultipart(), Error, BadResponse, ConvertFailed, EmptyAudio, FfmpegMissing, NoApiKey, ProviderError (+5 more)

### Community 34 - "config.zig"
Cohesion: 0.42
Nodes (8): Config, defaultVaultDir(), loadDotEnv(), optionalValue(), parseFloat(), parseInt(), truthy(), value()

### Community 35 - "jsonutil.zig"
Cohesion: 0.17
Nodes (6): get(), getBool(), getFloat(), getInt(), getString(), Pair

### Community 36 - "officeDir"
Cohesion: 0.24
Nodes (13): countFiles(), createFileForMcp(), deleteFileForMcp(), home(), Kind, docs, html, presentations (+5 more)

### Community 37 - "busqueda.zig"
Cohesion: 0.26
Nodes (13): containsIgnoreCase(), Item, lowerAscii(), maybeAdd(), notaTexto(), page(), pushLower(), Results (+5 more)

### Community 38 - "storeConverted"
Cohesion: 0.17
Nodes (15): driveExtension(), fileNameStem(), importDirectory(), importDriveRoute(), importErrorLabel(), importLocalRoute(), ImportSummary, kindLabel() (+7 more)

### Community 39 - "submenuWeight"
Cohesion: 0.20
Nodes (14): activityPercent(), activityRow(), axisGoalHtml(), axisMinNeeded(), axisTableHtml(), buildAxes(), collectAxes(), containsIgnoreCase() (+6 more)

### Community 40 - "subjectDetailHtml"
Cohesion: 0.16
Nodes (14): cacheAgeText(), gradeCard(), gradesGrid(), gradesKpis(), gradesPageHtml(), modoToggleHtml(), partialLabel(), shortCode() (+6 more)

### Community 41 - "esc"
Cohesion: 0.19
Nodes (14): calEventsMonth(), calTasksMonth(), calUpcoming(), esc(), examCard(), inMonth(), nextEvent(), notesList() (+6 more)

### Community 42 - "connections"
Cohesion: 0.20
Nodes (12): absExists(), connCard(), connections(), envValue(), homePathExists(), opencodeBin(), opencodeInfo(), opencodePairLink() (+4 more)

### Community 43 - "charts.js"
Cohesion: 0.30
Nodes (9): boot(), hide(), init(), markText(), place(), show(), tooltip(), wire() (+1 more)

### Community 44 - "el"
Cohesion: 0.30
Nodes (11): el(), fillMenu(), icon(), initSearch(), move(), paint(), run(), initThemePicker() (+3 more)

### Community 45 - "tests.zig"
Cohesion: 0.21
Nodes (6): collect(), diskFreeBytes(), meminfoKiB(), Memory, Metrics, readMemory()

### Community 46 - "charts.zig"
Cohesion: 0.31
Nodes (9): apilada(), Bar, barras(), clampPct(), donut(), esc(), Palette, progreso() (+1 more)

### Community 47 - "nativesync.zig"
Cohesion: 0.33
Nodes (9): build(), classItemsJson(), eventosJson(), gradesJson(), horarioJson(), inboxJson(), notasJson(), noteRowsJson() (+1 more)

### Community 48 - "std"
Cohesion: 0.24
Nodes (6): build(), linkSqlite(), accessTokenFor(), Evento, fetch(), Resultado

### Community 49 - "chartEmpty"
Cohesion: 0.22
Nodes (10): axisCat(), chartAvg(), chartAxesRadar(), chartEmpty(), chartGradeDist(), chartHealth(), chartNotes(), chartStackedSubjects() (+2 more)

### Community 50 - "portal"
Cohesion: 0.28
Nodes (9): findBell(), findCreate(), findSearch(), initBell(), portal(), closeFn(), openFn(), outside() (+1 more)

### Community 51 - "OfficeError"
Cohesion: 0.25
Nodes (8): OfficeError, ContentTooLarge, FileExists, InvalidSlug, InvalidTitle, IoError, NotFound, TitleTooLong

### Community 52 - "fetchSubjectDetail"
Cohesion: 0.33
Nodes (7): fetchSubjectDetail(), floatField(), formatDate(), lastSegmentInt(), loadSubmenus(), parseGrades(), realGradeAxes()

### Community 54 - "ImportStatus"
Cohesion: 0.33
Nodes (5): ImportStatus, empty, exists, failed, imported

### Community 55 - "kpi"
Cohesion: 0.33
Nodes (6): kpi(), kpiAvg(), kpiConv(), kpiNotes(), kpiTasks(), taskCounts()

### Community 56 - "syncThemeUI"
Cohesion: 0.53
Nodes (6): applyTheme(), currentTheme(), initThemeGuard(), reapply(), syncThemeUI(), themeById()

### Community 57 - "initModelPicker"
Cohesion: 0.60
Nodes (6): initModelPicker(), aplicarConConfirmacion(), esAvisoDePago(), pintar(), precioEtiqueta(), subDeModelo()

### Community 58 - "calendar_annotations"
Cohesion: 0.50
Nodes (3): calendar_annotations, idx_calendar_annotations_date, idx_calendar_annotations_kind_date

### Community 60 - "createUnique"
Cohesion: 0.60
Nodes (5): getNoteBySlug(), importedTitle(), saveImportedNote(), createUnique(), sluginator()

### Community 61 - "20240101000011_flashcards.sql"
Cohesion: 0.83
Nodes (3): flashcards, idx_flashcards_due, idx_flashcards_subject

### Community 62 - "ConnState"
Cohesion: 0.50
Nodes (4): ConnState, off, on, soft

### Community 65 - "TaskScope"
Cohesion: 0.67
Nodes (3): TaskScope, today, week

### Community 70 - "security.zig"
Cohesion: 0.24
Nodes (3): generateSessionToken(), hashSessionToken(), hexEncode()

### Community 71 - "presentationRoute"
Cohesion: 0.38
Nodes (7): getSlidePtr(), getSlidesPtr(), importPresentation(), presentationRoute(), reorderSlides(), savePresentation(), slideValue()

## Knowledge Gaps
- **90 isolated node(s):** `horario`, `tareas`, `evaluaciones`, `asistencia`, `perfil` (+85 more)
  These have ≤1 connection - possible missing edges. (Counts symbols only; 344 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **8 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **What connects `horario`, `tareas`, `evaluaciones` to the rest of the system?**
  _90 weakly-connected nodes found - possible missing edges._
- **Should `App` be split into smaller, more focused modules?**
  _Cohesion score 0.05711086226203808 - nodes in this community are weakly interconnected._
- **Should `notes.zig` be split into smaller, more focused modules?**
  _Cohesion score 0.07530864197530865 - nodes in this community are weakly interconnected._
- **Should `importer.zig` be split into smaller, more focused modules?**
  _Cohesion score 0.07539450613676213 - nodes in this community are weakly interconnected._
- **Should `mcp.zig` be split into smaller, more focused modules?**
  _Cohesion score 0.07706766917293233 - nodes in this community are weakly interconnected._
- **Should `sqlite.zig` be split into smaller, more focused modules?**
  _Cohesion score 0.07272727272727272 - nodes in this community are weakly interconnected._
- **Should `office.zig` be split into smaller, more focused modules?**
  _Cohesion score 0.08416389811738649 - nodes in this community are weakly interconnected._