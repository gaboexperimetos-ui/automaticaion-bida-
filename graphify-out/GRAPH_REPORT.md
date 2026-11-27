# Graph Report - styp  (2026-11-27)

## Corpus Check
- cluster-only mode — file stats not available

## Summary
- 1481 nodes · 3231 edges · 70 communities (62 shown, 8 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 42 edges (avg confidence: 0.85)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `ef6bcbee`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- Community 0
- Community 1
- Community 2
- Community 3
- Community 4
- Community 5
- Community 6
- Community 7
- Community 8
- Community 9
- Community 10
- Community 11
- Community 12
- Community 13
- Community 14
- Community 15
- Community 16
- Community 17
- Community 18
- Community 19
- Community 20
- Community 21
- Community 22
- Community 23
- Community 24
- Community 25
- Community 26
- Community 27
- Community 28
- Community 29
- Community 30
- Community 31
- Community 32
- Community 33
- Community 34
- Community 35
- Community 36
- Community 37
- Community 38
- Community 39
- Community 40
- Community 41
- Community 42
- Community 43
- Community 44
- Community 45
- Community 46
- Community 47
- Community 48
- Community 49
- Community 50
- Community 51
- Community 52
- Community 53
- Community 54
- Community 55
- Community 56
- Community 57
- Community 58
- Community 59
- Community 60
- Community 61
- Community 62
- Community 63
- Community 64
- Community 65
- Community 66
- Community 68

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

## Communities (70 total, 8 thin omitted)

### Community 0 - "Community 0"
Cohesion: 0.06
Nodes (31): academicTool(), AcademicView, asistencia, evaluaciones, horario, mensajes, perfil, tareas (+23 more)

### Community 1 - "Community 1"
Cohesion: 0.08
Nodes (78): annotationEnvelope(), annotations(), apiNoteDispatch(), apiNoteRoute(), apiNotes(), appendFtsCards(), appendFtsResults(), appendIcsLine() (+70 more)

### Community 2 - "Community 2"
Cohesion: 0.08
Nodes (57): appendCsvField(), appendDecoded(), appendOdsRow(), appendSheetRow(), attrValue(), cellValue(), cleanText(), columnIndex() (+49 more)

### Community 3 - "Community 3"
Cohesion: 0.08
Nodes (53): aiCost(), appStatus(), argI64(), backupDir(), backupNow(), backups(), calendarAdd(), calendarDelete() (+45 more)

### Community 4 - "Community 4"
Cohesion: 0.07
Nodes (29): Db, Error, OutOfMemory, SqliteError, sqlite3_backup_finish(), sqlite3_backup_init(), sqlite3_backup_step(), sqlite3_bind_double() (+21 more)

### Community 5 - "Community 5"
Cohesion: 0.07
Nodes (48): appendCalendarSection(), appendGradeSection(), appendIdentitySection(), appendNotesSection(), appendSlugCodepoint(), appendTasksSection(), buildStudentReport(), CellRef (+40 more)

### Community 6 - "Community 6"
Cohesion: 0.10
Nodes (45): academicEventsIcs(), annotationOn(), annotationTime(), Anotacion, appendChips(), appendDay(), appendHeader(), appendIcsLine() (+37 more)

### Community 7 - "Community 7"
Cohesion: 0.06
Nodes (36): applyCached(), applyFull(), Axis, CachedFull, chartHeatmap(), CivilDate, Clase, collectPartialIds() (+28 more)

### Community 8 - "Community 8"
Cohesion: 0.08
Nodes (35): RFC-5545, addDays(), dateString(), identityKeys(), localParts(), pad(), parseDateTime(), parseEvent() (+27 more)

### Community 9 - "Community 9"
Cohesion: 0.12
Nodes (29): appendFrame(), budgetExhausted(), callProvider(), chat(), ChatMessage, conversationMessages(), conversations(), cost() (+21 more)

### Community 10 - "Community 10"
Cohesion: 0.07
Nodes (32): Accion, next, pause, play, previous, seek, toggle, call() (+24 more)

### Community 11 - "Community 11"
Cohesion: 0.09
Nodes (26): Config, defaultVaultDir(), loadDotEnv(), optionalValue(), parseFloat(), parseInt(), truthy(), value() (+18 more)

### Community 12 - "Community 12"
Cohesion: 0.13
Nodes (32): addChatMessage(), applySettingsVisuals(), bindDropdown(), closeDropdowns(), closeModal(), createOffice(), esc(), fieldHTML() (+24 more)

### Community 13 - "Community 13"
Cohesion: 0.13
Nodes (11): json_body(), main(), MockMcpHandler, NoRedirect, request(), run_auth_smoke(), run_external_integrations_smoke(), run_mainline_merge_smoke() (+3 more)

### Community 14 - "Community 14"
Cohesion: 0.09
Nodes (26): academicText(), collect(), count(), Counts, esc(), fetchNotes(), filterLink(), fromClassroom() (+18 more)

### Community 15 - "Community 15"
Cohesion: 0.12
Nodes (27): Card, cardFrom(), cardValue(), Civil, civilFromDays(), createCard(), daysFromCivil(), getCard() (+19 more)

### Community 16 - "Community 16"
Cohesion: 0.12
Nodes (25): authorizationUrl(), buildStateCookie(), calendarEvents(), clearStateCookie(), Config, Error, BadResponse, EmailMissing (+17 more)

### Community 17 - "Community 17"
Cohesion: 0.13
Nodes (24): agendaItems(), buildRows(), Clase, clasesDe(), classAt(), datePart(), dayHasClasses(), Dia (+16 more)

### Community 18 - "Community 18"
Cohesion: 0.10
Nodes (21): Asset, assetByPath(), find(), attrValue(), clearUnknownSlots(), fillPage(), loginPage(), loginTemplate() (+13 more)

### Community 19 - "Community 19"
Cohesion: 0.14
Nodes (22): accessTokenFor(), Announcement, Course, CoursesResult, Detail, detailHtml(), dueLabel(), fetchAnnouncements() (+14 more)

### Community 20 - "Community 20"
Cohesion: 0.14
Nodes (24): aiCatchPrompt(), boot(), importObsidian(), initAiLegacy(), initAssistant(), buildPrompt(), ctx(), initChatWorkspace() (+16 more)

### Community 21 - "Community 21"
Cohesion: 0.08
Nodes (23): name, runs-on, steps, concurrency, cancel-in-progress, group, GH_TOKEN, jobs (+15 more)

### Community 22 - "Community 22"
Cohesion: 0.08
Nodes (23): concurrency, cancel-in-progress, group, jobs, validate, name, on, pull_request (+15 more)

### Community 23 - "Community 23"
Cohesion: 0.22
Nodes (22): initCalendar(), dayPanel(), deleteEntry(), describeIcsEvent(), editEntry(), ensureCalendarIcsInput(), gridHost(), importCalendarIcs() (+14 more)

### Community 24 - "Community 24"
Cohesion: 0.12
Nodes (16): app_config, notes, sessions, users, idx_note_versions_note_id, note_versions, notes_fts, idx_sessions_expires_at (+8 more)

### Community 25 - "Community 25"
Cohesion: 0.23
Nodes (17): asistenciaHtml(), esc(), examenesHtml(), fecha(), fila(), horarioHtml(), kpi(), mensajeItem() (+9 more)

### Community 26 - "Community 26"
Cohesion: 0.17
Nodes (16): applyWindowCss(), buildWindow(), configDir(), configPath(), GdkEventKey, injectGlassCss(), loadGeometry(), main() (+8 more)

### Community 27 - "Community 27"
Cohesion: 0.34
Nodes (21): documentRoute(), documents(), download(), errorPage(), fileExists(), formField(), handle(), htmlPageRoute() (+13 more)

### Community 28 - "Community 28"
Cohesion: 0.21
Nodes (16): done(), initCollections(), initCollectionsSafe(), initMusicPlayer(), musicAddUrl(), musicAll(), musicCurrent(), musicPlay() (+8 more)

### Community 29 - "Community 29"
Cohesion: 0.16
Nodes (17): accessTokenFor(), Contact, contacts(), ContactsResult, docKindLabel(), docSectionHtml(), DocText, downloadFile() (+9 more)

### Community 30 - "Community 30"
Cohesion: 0.21
Nodes (19): initNotesEditor(), buildSaveButton(), debouncedRender(), filterLibrary(), hydrate(), insert(), linePrefix(), markDirty() (+11 more)

### Community 31 - "Community 31"
Cohesion: 0.20
Nodes (17): editPresentation(), formatHumanSize(), getSlidePtr(), getSlidesPtr(), humanSizeFromPath(), importResultPage(), libraryPage(), listPage() (+9 more)

### Community 32 - "Community 32"
Cohesion: 0.14
Nodes (17): calGrid(), capitalize(), chartLoad14(), chartWeek(), civilFromDays(), classesList(), daysFromCivil(), diasDelMes() (+9 more)

### Community 33 - "Community 33"
Cohesion: 0.14
Nodes (13): buildMultipart(), Error, BadResponse, ConvertFailed, EmptyAudio, FfmpegMissing, NoApiKey, ProviderError (+5 more)

### Community 34 - "Community 34"
Cohesion: 0.22
Nodes (13): CachedProbe, callTool(), callToolAllowError(), deepestString(), headerValue(), listTools(), parseRpcMessage(), probe() (+5 more)

### Community 35 - "Community 35"
Cohesion: 0.17
Nodes (6): get(), getBool(), getFloat(), getInt(), getString(), Pair

### Community 36 - "Community 36"
Cohesion: 0.24
Nodes (13): countFiles(), createFileForMcp(), deleteFileForMcp(), home(), Kind, docs, html, presentations (+5 more)

### Community 37 - "Community 37"
Cohesion: 0.26
Nodes (13): containsIgnoreCase(), Item, lowerAscii(), maybeAdd(), notaTexto(), page(), pushLower(), Results (+5 more)

### Community 38 - "Community 38"
Cohesion: 0.20
Nodes (13): driveExtension(), fileNameStem(), importDirectory(), importDriveRoute(), importErrorLabel(), importLocalRoute(), ImportSummary, kindLabel() (+5 more)

### Community 39 - "Community 39"
Cohesion: 0.20
Nodes (14): activityPercent(), activityRow(), axisGoalHtml(), axisMinNeeded(), axisTableHtml(), buildAxes(), collectAxes(), containsIgnoreCase() (+6 more)

### Community 40 - "Community 40"
Cohesion: 0.16
Nodes (14): cacheAgeText(), gradeCard(), gradesGrid(), gradesKpis(), gradesPageHtml(), modoToggleHtml(), partialLabel(), shortCode() (+6 more)

### Community 41 - "Community 41"
Cohesion: 0.19
Nodes (14): calEventsMonth(), calTasksMonth(), calUpcoming(), esc(), examCard(), inMonth(), nextEvent(), notesList() (+6 more)

### Community 42 - "Community 42"
Cohesion: 0.20
Nodes (12): absExists(), connCard(), connections(), envValue(), homePathExists(), opencodeBin(), opencodeInfo(), opencodePairLink() (+4 more)

### Community 43 - "Community 43"
Cohesion: 0.30
Nodes (9): boot(), hide(), init(), markText(), place(), show(), tooltip(), wire() (+1 more)

### Community 44 - "Community 44"
Cohesion: 0.30
Nodes (11): el(), fillMenu(), icon(), initSearch(), move(), paint(), run(), initThemePicker() (+3 more)

### Community 45 - "Community 45"
Cohesion: 0.25
Nodes (8): build(), linkSqlite(), collect(), diskFreeBytes(), meminfoKiB(), Memory, Metrics, readMemory()

### Community 46 - "Community 46"
Cohesion: 0.31
Nodes (9): apilada(), Bar, barras(), clampPct(), donut(), esc(), Palette, progreso() (+1 more)

### Community 47 - "Community 47"
Cohesion: 0.33
Nodes (9): build(), classItemsJson(), eventosJson(), gradesJson(), horarioJson(), inboxJson(), notasJson(), noteRowsJson() (+1 more)

### Community 48 - "Community 48"
Cohesion: 0.22
Nodes (4): accessTokenFor(), Evento, fetch(), Resultado

### Community 49 - "Community 49"
Cohesion: 0.22
Nodes (10): axisCat(), chartAvg(), chartAxesRadar(), chartEmpty(), chartGradeDist(), chartHealth(), chartNotes(), chartStackedSubjects() (+2 more)

### Community 50 - "Community 50"
Cohesion: 0.28
Nodes (9): findBell(), findCreate(), findSearch(), initBell(), portal(), closeFn(), openFn(), outside() (+1 more)

### Community 51 - "Community 51"
Cohesion: 0.25
Nodes (8): OfficeError, ContentTooLarge, FileExists, InvalidSlug, InvalidTitle, IoError, NotFound, TitleTooLong

### Community 52 - "Community 52"
Cohesion: 0.33
Nodes (7): fetchSubjectDetail(), floatField(), formatDate(), lastSegmentInt(), loadSubmenus(), parseGrades(), realGradeAxes()

### Community 54 - "Community 54"
Cohesion: 0.33
Nodes (5): ImportStatus, empty, exists, failed, imported

### Community 55 - "Community 55"
Cohesion: 0.33
Nodes (6): kpi(), kpiAvg(), kpiConv(), kpiNotes(), kpiTasks(), taskCounts()

### Community 56 - "Community 56"
Cohesion: 0.53
Nodes (6): applyTheme(), currentTheme(), initThemeGuard(), reapply(), syncThemeUI(), themeById()

### Community 57 - "Community 57"
Cohesion: 0.60
Nodes (6): initModelPicker(), aplicarConConfirmacion(), esAvisoDePago(), pintar(), precioEtiqueta(), subDeModelo()

### Community 58 - "Community 58"
Cohesion: 0.50
Nodes (3): calendar_annotations, idx_calendar_annotations_date, idx_calendar_annotations_kind_date

### Community 60 - "Community 60"
Cohesion: 0.60
Nodes (5): getNoteBySlug(), importedTitle(), saveImportedNote(), createUnique(), sluginator()

### Community 61 - "Community 61"
Cohesion: 0.83
Nodes (3): flashcards, idx_flashcards_due, idx_flashcards_subject

### Community 62 - "Community 62"
Cohesion: 0.50
Nodes (4): ConnState, off, on, soft

### Community 65 - "Community 65"
Cohesion: 0.67
Nodes (3): TaskScope, today, week

## Knowledge Gaps
- **90 isolated node(s):** `asistencia`, `evaluaciones`, `horario`, `mensajes`, `perfil` (+85 more)
  These have ≤1 connection - possible missing edges. (Counts symbols only; 344 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **8 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **What connects `asistencia`, `evaluaciones`, `horario` to the rest of the system?**
  _90 weakly-connected nodes found - possible missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.05765271105010295 - nodes in this community are weakly interconnected._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.07530864197530865 - nodes in this community are weakly interconnected._
- **Should `Community 2` be split into smaller, more focused modules?**
  _Cohesion score 0.07539450613676213 - nodes in this community are weakly interconnected._
- **Should `Community 3` be split into smaller, more focused modules?**
  _Cohesion score 0.07706766917293233 - nodes in this community are weakly interconnected._
- **Should `Community 4` be split into smaller, more focused modules?**
  _Cohesion score 0.07272727272727272 - nodes in this community are weakly interconnected._
- **Should `Community 5` be split into smaller, more focused modules?**
  _Cohesion score 0.06980392156862746 - nodes in this community are weakly interconnected._