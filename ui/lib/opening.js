// The opening a page's edit belongs to (ADR 0049): a delayed write (a
// debounced pattern, a panel edit, a patch, a move) names the generation of
// the project open when its edit began, so the host refuses it
// (`project-changed`) once another project, or a new opening of the same
// one, is open, even when the files are byte for byte the same. No DOM.

/** The open project's generation a connection last heard of (ADR 0049), or null. */
export function generationOf(connection) {
  const generation = connection?.generation ?? connection?.description?.generation;
  return Number.isSafeInteger(generation) ? generation : null;
}

/** The generation a `graph` message gives, or null. */
export function graphGeneration(graph) {
  return Number.isSafeInteger(graph?.generation) ? graph.generation : null;
}

/** `message` naming `generation` when one is known; as it is otherwise (the host takes the open project). */
export function named(message, generation) {
  return Number.isSafeInteger(generation) ? { ...message, generation } : message;
}
