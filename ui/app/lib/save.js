// Save (M2.8-11): edits are written on release, so Save writes again every
// parameter still unsaved, through the existing retry, and says "saved" or
// what was not. A .signal project (M2.8-12) is then written back to its
// file (`project.save`). Plain ES module, no DOM.

/** The `retry` of every unsaved control (`unsaved` as the host pushes it). */
export function saveRequests(unsaved) {
  return (unsaved?.parameters ?? []).filter((p) => p.control).map((p) => ({ type: 'retry', address: p.address }));
}

/**
 * What Save reports: `results` are the retries' outcomes, `{address,
 * error}` with the refusal (`{code, message}`) or none. `not-unsaved` means
 * the file already holds it. A live edit of a plan port is not a key of
 * project.toml, so it cannot be saved.
 */
export function saveReport(unsaved, results) {
  const problems = results
    .filter((r) => r.error && r.error.code !== 'not-unsaved')
    .map((r) => `${r.address}: ${r.error.message ?? r.error.code}`);
  for (const p of (unsaved?.parameters ?? []).filter((p) => !p.control)) {
    problems.push(`${p.address}: a live edit of a port inside the plan, not in project.toml`);
  }
  return problems.length ? { saved: false, text: `not saved: ${problems.join('; ')}` } : { saved: true, text: 'saved' };
}

/** After the retries, a .signal project's write to its file; none for a folder project. */
export function archiveSave(project) {
  return project?.file ? { type: 'project.save' } : null;
}
