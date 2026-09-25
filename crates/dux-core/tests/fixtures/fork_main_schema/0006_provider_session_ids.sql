alter table agent_sessions
    add column provider_session_ids text not null default '{}';
