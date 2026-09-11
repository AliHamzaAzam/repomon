CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT, model TEXT, started_at REAL, cwd TEXT);
CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL);
INSERT INTO sessions VALUES ('hermes-fixture','tui','hermes-model',1789117200,NULL), ('hermes-other','tui','hermes-model',1789117200,NULL);
INSERT INTO messages VALUES (1,'hermes-fixture','user','Please inspect the Hermes fixture repository carefully',NULL,NULL,NULL,1789117201);
INSERT INTO messages VALUES (2,'hermes-fixture','assistant','The Hermes fixture repository inspection is complete',NULL,'[{"id":"call-1","type":"function","function":{"name":"terminal","arguments":"{\"command\":\"pwd\"}"}}]',NULL,1789117202);
INSERT INTO messages VALUES (3,'hermes-fixture','tool','/fixture','call-1',NULL,'terminal',1789117203);
INSERT INTO messages VALUES (4,'hermes-other','user','Unrelated session must never appear',NULL,NULL,NULL,1789117204);
