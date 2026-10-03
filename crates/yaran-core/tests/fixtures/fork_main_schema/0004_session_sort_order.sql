-- Audit03: manual sidebar ordering for agent sessions.
--
-- Existing rows are back-filled to preserve the previous display order:
-- most recently updated first, with id as a deterministic tie-breaker.
-- New code thereafter treats this as the user-controlled order.

alter table agent_sessions add column sort_order integer not null default 0;

update agent_sessions
set sort_order = (
    select count(*)
    from agent_sessions as prior
    where prior.updated_at > agent_sessions.updated_at
       or (prior.updated_at = agent_sessions.updated_at and prior.id < agent_sessions.id)
);

create index if not exists idx_agent_sessions_sort_order
    on agent_sessions(sort_order, updated_at desc, id);
