-- OpenWebUI Functions (§6.5d): every manifest gets a kind — 'tools' (the
-- original `class Tools` skills, model-callable methods), 'filter'
-- (`class Filter` inlet/outlet middleware) or 'pipe' (`class Pipe`
-- pseudo-models). Existing rows are skills.
ALTER TABLE owui_tools ADD COLUMN kind TEXT NOT NULL DEFAULT 'tools';