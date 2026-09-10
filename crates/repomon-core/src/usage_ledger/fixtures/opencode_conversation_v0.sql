-- Provider schema/part shapes already read by agent/opencode.rs; synthetic content and IDs.
CREATE TABLE session(id TEXT PRIMARY KEY, directory TEXT, title TEXT);
CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
CREATE INDEX message_session_time ON message(session_id, time_created);
CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
CREATE INDEX part_message ON part(message_id, time_created);
INSERT INTO session VALUES('first','/synthetic/first','Fix preview');
INSERT INTO session VALUES('second','/synthetic/second','Private other session');
INSERT INTO message VALUES('user','first',1000,'{"role":"user"}');
INSERT INTO message VALUES('answer','first',1001,'{"role":"assistant","modelID":"model-example","tokens":{"input":10,"output":20}}');
INSERT INTO message VALUES('other','second',1001,'{"role":"assistant","modelID":"model-example","tokens":{"input":30,"output":40}}');
INSERT INTO part VALUES('prompt','user','first',1000,'{"type":"text","text":"Fix the preview"}');
INSERT INTO part VALUES('prose','answer','first',1001,'{"type":"text","text":"The preview is fixed."}');
INSERT INTO part VALUES('tool','answer','first',1002,'{"type":"tool","tool":"read","callID":"call-1","state":{"status":"completed","input":{"file_path":"view.rs"},"output":"file content"}}');
INSERT INTO part VALUES('finish','answer','first',1003,'{"type":"step-finish"}');
INSERT INTO part VALUES('private','other','second',1001,'{"type":"text","text":"DO NOT MIX SESSIONS"}');
