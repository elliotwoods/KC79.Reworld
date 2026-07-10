// Build an event predicate from query-string parameters.

export function buildFilter(query) {
  const type = query.type || null;
  const col = query.col != null && query.col !== '' ? Number(query.col) : null;
  const portal = query.portal != null && query.portal !== '' ? Number(query.portal) : null;
  const minLevel = query.min_level != null && query.min_level !== '' ? Number(query.min_level) : null;
  const from = query.from ? Number(query.from) : null;
  const to = query.to ? Number(query.to) : null;
  const types = type ? new Set(type.split(',')) : null;

  return (ev) => {
    if (types && !types.has(ev.type)) return false;
    if (col != null && ev.col !== col) return false;
    if (portal != null && ev.portal !== portal && ev.source !== portal) return false;
    if (minLevel != null && (ev.level ?? 0) < minLevel) return false;
    if (from != null && ev.ts < from) return false;
    if (to != null && ev.ts > to) return false;
    return true;
  };
}
