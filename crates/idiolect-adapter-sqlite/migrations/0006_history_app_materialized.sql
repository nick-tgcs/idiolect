-- History rows are now materialized by the application layer (so the text can
-- be encrypted at rest) instead of by SQL triggers. Drop the triggers added in
-- 0004; the table and indexes are unchanged.
DROP TRIGGER IF EXISTS trg_ime_text_history_on_commit;
DROP TRIGGER IF EXISTS trg_ime_text_history_on_cancel;
