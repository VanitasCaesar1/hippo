-- synthetic: a database from a future OpenCode whose session_message has no
-- seq column, which hippo reads. Loaded on its own, without schema.sql.
CREATE TABLE `migration` (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
INSERT INTO migration (id, time_completed) VALUES ('20261030054400_job', 1793500000000);
INSERT INTO migration (id, time_completed) VALUES ('20991231000000_synthetic_future', 1793500000000);
CREATE TABLE `kv` (`key` text PRIMARY KEY, `value` text NOT NULL, `time_created` integer NOT NULL, `time_updated` integer NOT NULL);
INSERT INTO kv (key, value, time_created, time_updated) VALUES ('synthetic', 'true', 1793500000000, 1793500000000);
CREATE TABLE `session_v2` (`id` text PRIMARY KEY, `project_id` text NOT NULL, `parent_id` text, `fork_session_id` text, `fork_boundary` text, `directory` text NOT NULL, `title` text, `time_created` integer NOT NULL, `time_updated` integer NOT NULL);
INSERT INTO session_v2 (id, project_id, directory, title, time_created, time_updated) VALUES ('ses_9999synthetic0000000009', 'prj_synthetic', '/home/synthetic/shop', 'From a future OpenCode', 1793500000000, 1793500000000);
CREATE TABLE `session_message` (`id` text PRIMARY KEY, `session_id` text NOT NULL, `type` text NOT NULL, `position` integer NOT NULL, `time_created` integer NOT NULL, `time_updated` integer NOT NULL, `data` text NOT NULL);
INSERT INTO session_message (id, session_id, type, position, time_created, time_updated, data) VALUES ('msg_9999_001', 'ses_9999synthetic0000000009', 'user', 1, 1793500000000, 1793500000000, '{"text":"NEWER FORMAT, NOT READ"}');
