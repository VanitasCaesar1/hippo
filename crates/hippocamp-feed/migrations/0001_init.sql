-- hippocamp-feed schema, version 1.
-- Times are fixed-width text (YYYY-MM-DDTHH:MM:SS.ffffffZ); booleans are 0 or 1;
-- JSON is text. Nothing here stores a computed count, rank or score.

CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  github_id INTEGER UNIQUE,
  login TEXT NOT NULL UNIQUE COLLATE NOCASE,
  github_created_at TEXT,
  created_at TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'none' CHECK (role IN ('none','moderator')),
  restricted_until TEXT,
  banned INTEGER NOT NULL DEFAULT 0,
  deleted_at TEXT,
  theme TEXT NOT NULL DEFAULT 'auto',
  personal_css TEXT NOT NULL DEFAULT ''
);

CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  csrf TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL
);

CREATE TABLE oauth_states (
  state_hash TEXT PRIMARY KEY,
  verifier TEXT NOT NULL,
  return_to TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE packs (
  name TEXT PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'passes' CHECK (kind IN ('passes','theme')),
  version TEXT NOT NULL,
  description TEXT NOT NULL,
  authors TEXT NOT NULL,
  owner TEXT NOT NULL,
  owner_id INTEGER NOT NULL,
  maintainers TEXT NOT NULL,
  first_party INTEGER NOT NULL,
  license TEXT NOT NULL,
  repo TEXT NOT NULL,
  repo_id INTEGER NOT NULL,
  path TEXT NOT NULL,
  commit_id TEXT NOT NULL,
  pack_hash TEXT NOT NULL,
  passes TEXT NOT NULL,
  theme_contract INTEGER,
  theme_sha256 TEXT,
  review_id TEXT NOT NULL,
  review_at TEXT NOT NULL,
  listed INTEGER NOT NULL,
  synced_at TEXT NOT NULL
);

CREATE TABLE pack_contents (
  pack TEXT NOT NULL REFERENCES packs(name),
  pack_hash TEXT NOT NULL,
  version TEXT NOT NULL,
  commit_id TEXT NOT NULL,
  first_seen TEXT NOT NULL,
  PRIMARY KEY (pack, pack_hash)
);

CREATE TABLE pack_maintainer_ids (
  pack TEXT NOT NULL REFERENCES packs(name),
  github_id INTEGER NOT NULL,
  login TEXT NOT NULL,
  bound_at TEXT NOT NULL,
  PRIMARY KEY (pack, github_id)
);

-- An accepted stylesheet (css) or the reason a content was refused (failure).
CREATE TABLE theme_files (
  pack TEXT NOT NULL REFERENCES packs(name),
  pack_hash TEXT NOT NULL,
  css TEXT,
  failure TEXT,
  at TEXT NOT NULL,
  PRIMARY KEY (pack, pack_hash),
  CHECK ((css IS NULL) <> (failure IS NULL))
);

CREATE TABLE posts (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('pack','discussion')),
  author_id INTEGER REFERENCES users(id),
  pack TEXT UNIQUE REFERENCES packs(name),
  title TEXT NOT NULL,
  link TEXT,
  body TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  edited_at TEXT,
  state TEXT NOT NULL DEFAULT 'visible' CHECK (state IN ('visible','hidden','removed','deleted')),
  removed_reason TEXT,
  locked INTEGER NOT NULL DEFAULT 0,
  CHECK ((kind = 'pack') = (pack IS NOT NULL))
);
CREATE INDEX posts_created_at ON posts(created_at);
CREATE INDEX posts_author_id ON posts(author_id);

CREATE TABLE comments (
  id INTEGER PRIMARY KEY,
  post_id INTEGER NOT NULL REFERENCES posts(id),
  parent_id INTEGER REFERENCES comments(id),
  depth INTEGER NOT NULL CHECK (depth BETWEEN 0 AND 10),
  author_id INTEGER REFERENCES users(id),
  body TEXT NOT NULL,
  created_at TEXT NOT NULL,
  edited_at TEXT,
  state TEXT NOT NULL DEFAULT 'visible' CHECK (state IN ('visible','hidden','removed','deleted')),
  removed_reason TEXT
);
CREATE INDEX comments_post_id ON comments(post_id);
CREATE INDEX comments_author_id ON comments(author_id);
CREATE INDEX comments_created_at ON comments(created_at);

CREATE TABLE votes (
  user_id INTEGER NOT NULL REFERENCES users(id),
  target_type TEXT NOT NULL CHECK (target_type IN ('post','comment')),
  target_id INTEGER NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (user_id, target_type, target_id)
);
CREATE INDEX votes_target ON votes(target_type, target_id);

CREATE TABLE hides (
  user_id INTEGER NOT NULL REFERENCES users(id),
  post_id INTEGER NOT NULL REFERENCES posts(id),
  at TEXT NOT NULL,
  PRIMARY KEY (user_id, post_id)
);

CREATE TABLE measurements (
  id INTEGER PRIMARY KEY,
  pack TEXT NOT NULL,
  pack_hash TEXT NOT NULL,
  user_id INTEGER NOT NULL REFERENCES users(id),
  eval_set TEXT NOT NULL,
  planted INTEGER NOT NULL CHECK (planted >= 5),
  kept INTEGER NOT NULL CHECK (kept >= 5),
  runs INTEGER NOT NULL CHECK (runs BETWEEN 3 AND 10),
  corrected_without INTEGER NOT NULL,
  damaged_without INTEGER NOT NULL,
  corrected_with INTEGER NOT NULL,
  damaged_with INTEGER NOT NULL,
  hippocamp TEXT NOT NULL,
  model TEXT NOT NULL,
  report_url TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  revisions INTEGER NOT NULL DEFAULT 0,
  invalid INTEGER NOT NULL DEFAULT 0,
  UNIQUE (pack, pack_hash, user_id),
  FOREIGN KEY (pack, pack_hash) REFERENCES pack_contents(pack, pack_hash)
);

CREATE TABLE reports (
  id INTEGER PRIMARY KEY,
  reporter_id INTEGER REFERENCES users(id),
  system INTEGER NOT NULL DEFAULT 0,
  target_type TEXT NOT NULL CHECK (target_type IN ('post','comment','measurement','user')),
  target_id INTEGER NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('spam','abuse','malicious','licence','misleading','other','ring')),
  note TEXT NOT NULL DEFAULT '',
  snapshot TEXT NOT NULL DEFAULT '',
  details TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','upheld','dismissed')),
  resolved_by INTEGER REFERENCES users(id),
  resolved_at TEXT
);
CREATE INDEX reports_status ON reports(status);
CREATE UNIQUE INDEX reports_one_per_reporter ON reports(reporter_id, target_type, target_id)
  WHERE reporter_id IS NOT NULL;

CREATE TABLE audit_log (
  id INTEGER PRIMARY KEY,
  at TEXT NOT NULL,
  actor_id INTEGER REFERENCES users(id),
  action TEXT NOT NULL,
  target_type TEXT,
  target_id TEXT,
  reason TEXT NOT NULL DEFAULT '',
  public INTEGER NOT NULL,
  details TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_log_at ON audit_log(at);
CREATE INDEX audit_log_action_target ON audit_log(action, target_id);

CREATE TRIGGER audit_log_no_update BEFORE UPDATE ON audit_log
BEGIN
  SELECT RAISE(ABORT, 'audit_log is append-only');
END;

CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON audit_log
BEGIN
  SELECT RAISE(ABORT, 'audit_log is append-only');
END;

-- Keys: created_at, last_sync, last_sync_ok, last_successful_sync, last_backup.
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
