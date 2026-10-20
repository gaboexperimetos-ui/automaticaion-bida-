-- Agent (búsqueda web): índice FTS5 dedicado para `GET /api/search`.
--
-- POR QUÉ NO SE REUTILIZA `notes_fts` (migración 20240101000002_knowledge.sql)
-- ==========================================================================
-- `notes_fts` YA existe en esta base y la mantiene el código de aplicación
-- (src/knowledge/notes.rs): hace INSERT/DELETE manual con el cuerpo real de
-- cada nota (leído del .md en disco). Además:
--
--   * `notes` NO tiene columna `content` — el cuerpo vive en el .md y en
--     `note_versions.content`. Un trigger con `new.content` revienta con
--     "no such column: new.content" y ROMPE el INSERT/UPDATE de las notas.
--   * Un trigger que insertara en `notes_fts` provocaría que el INSERT del
--     servicio falle con "constraint failed" (rowid duplicado en FTS5) y el
--     cuerpo real se perdería del índice de `knowledge`.
--
-- Por eso este índice es NUEVO, exclusivo de la búsqueda web, y lo mantiene
-- la propia base con triggers. No se hace DROP de nada existente.
--
-- CONTENIDO INDEXADO: título (columna propia) + tags + cuerpo de la última
-- versión (`note_versions.content`), con fallback a `notes.summary`.
-- La búsqueda es por prefijo: el handler sanea la query y añade `*`.
--
-- La tabla `notes_fts` original y su servicio quedan intactos.

CREATE VIRTUAL TABLE IF NOT EXISTS notes_search_fts USING fts5(
    title,
    content,
    tokenize = 'unicode61'
);

-- INSERT: indexa la nota recién creada. En este momento todavía no existe su
-- fila en `note_versions` (el servicio la inserta después), así que el
-- cuerpo indexado es el summary; el trigger de `note_versions` lo completa.
CREATE TRIGGER IF NOT EXISTS notes_search_fts_ai AFTER INSERT ON notes BEGIN
    INSERT OR REPLACE INTO notes_search_fts(rowid, title, content)
    VALUES (
        new.id,
        new.title,
        COALESCE(new.summary, '') || ' ' || COALESCE(new.tags, '')
    );
END;

-- UPDATE: refresca título y contenido. `UPDATE ... WHERE rowid` conserva el
-- resto de las columnas indexadas; el INSERT condicional cubre el caso en el
-- que la nota todavía no estaba indexada.
CREATE TRIGGER IF NOT EXISTS notes_search_fts_au AFTER UPDATE ON notes BEGIN
    UPDATE notes_search_fts
       SET title = new.title,
           content = COALESCE(
               (SELECT nv.content FROM note_versions nv
                 WHERE nv.note_id = new.id ORDER BY nv.id DESC LIMIT 1),
               new.summary, ''
           ) || ' ' || COALESCE(new.tags, '')
     WHERE rowid = new.id;

    INSERT OR REPLACE INTO notes_search_fts(rowid, title, content)
    SELECT new.id,
           new.title,
           COALESCE(
               (SELECT nv.content FROM note_versions nv
                 WHERE nv.note_id = new.id ORDER BY nv.id DESC LIMIT 1),
               new.summary, ''
           ) || ' ' || COALESCE(new.tags, '')
     WHERE NOT EXISTS (SELECT 1 FROM notes_search_fts WHERE rowid = new.id);
END;

-- DELETE: la nota desaparece también del índice.
CREATE TRIGGER IF NOT EXISTS notes_search_fts_ad AFTER DELETE ON notes BEGIN
    DELETE FROM notes_search_fts WHERE rowid = old.id;
END;

-- El servicio guarda el cuerpo de la nota en `note_versions` DESPUÉS de
-- insertar/actualizar la fila de `notes`; este trigger refresca el cuerpo
-- indexado cuando llega la versión nueva (si la nota no estaba indexada, la
-- inserta).
CREATE TRIGGER IF NOT EXISTS notes_search_fts_vi AFTER INSERT ON note_versions BEGIN
    UPDATE notes_search_fts
       SET content = COALESCE(new.content, '') || ' ' || COALESCE(
               (SELECT n.tags FROM notes n WHERE n.id = new.note_id), ''
           )
     WHERE rowid = new.note_id;

    INSERT OR REPLACE INTO notes_search_fts(rowid, title, content)
    SELECT n.id,
           n.title,
           COALESCE(new.content, '') || ' ' || COALESCE(n.tags, '')
      FROM notes n
     WHERE n.id = new.note_id
       AND NOT EXISTS (SELECT 1 FROM notes_search_fts WHERE rowid = new.note_id);
END;

-- BACKFILL: indexa las notas que ya existían en la base. Sólo se insertan las
-- que faltan (no se pisa lo ya indexado). El cuerpo se toma de la última
-- versión guardada; si no hay, del summary.
INSERT INTO notes_search_fts(rowid, title, content)
SELECT n.id,
       n.title,
       COALESCE(
           (SELECT nv.content FROM note_versions nv
             WHERE nv.note_id = n.id ORDER BY nv.id DESC LIMIT 1),
           n.summary, ''
       ) || ' ' || COALESCE(n.tags, '')
  FROM notes n
 WHERE NOT EXISTS (SELECT 1 FROM notes_search_fts WHERE rowid = n.id);
