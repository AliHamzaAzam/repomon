SELECT e.agent_kind, e.session_id,
                        SUM(e.input_tokens), SUM(e.output_tokens), SUM(e.cache_read_tokens),
                        SUM(e.cache_write_tokens), SUM(e.thinking_tokens),
                        SUM(CASE WHEN e.estimated = 1 THEN e.input_tokens + e.output_tokens
                                 + e.cache_read_tokens + e.cache_write_tokens ELSE 0 END),
                        SUM(CASE WHEN e.subagent = 1 THEN e.input_tokens + e.output_tokens
                                 + e.cache_read_tokens + e.cache_write_tokens ELSE 0 END),
                        COUNT(*), MIN(e.at), MAX(e.at), MAX(e.estimated), MAX(e.external),
                        MAX(e.repo_id), MAX(e.lane_id), MAX(e.cwd),
                        (SELECT x.model FROM usage_events x
                          WHERE x.agent_kind = e.agent_kind AND x.session_id = e.session_id
                          GROUP BY x.model
                          ORDER BY SUM(x.input_tokens + x.output_tokens) DESC LIMIT 1),
                        s.headline, s.turns, s.tool_calls, s.retries, s.headline_raw
                 FROM usage_events e
                 LEFT JOIN usage_sessions s
                   ON s.agent_kind = e.agent_kind AND s.session_id = e.session_id
                 WHERE e.at >= ?1 AND e.at < ?2 AND e.session_id IS NOT NULL
                   
                 GROUP BY e.agent_kind, e.session_id
                 ORDER BY MAX(e.at) DESC
                 LIMIT ?4
