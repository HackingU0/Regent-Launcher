// Run: node --experimental-strip-types app/src/queue.check.ts
import assert from "node:assert/strict";

const q = (await import(new URL("./queue.ts", import.meta.url).href)) as typeof import("./queue");

const s0 = { active: 0, playsets: [{ name: "A" }, { name: "B" }] };
const s1 = { ...s0, active: 1 };

// switch-then-toggle: playsetUse finishes with 2 pending (itself + the toggle) and is not optimistic -> shown now,
// so the queued toggle reads At = B. An optimistic toggle's own answer would have waited.
assert.equal(q.showResultNow(false, 2), true);
let shown = s0;
if (q.showResultNow(false, 2)) shown = s1;
assert.deepEqual(q.atFor(shown), { index: 1, name: "B" });
assert.equal(q.showResultNow(true, 2), false); // optimistic with a later edit queued: keep the later drawing
assert.equal(q.showResultNow(true, 1), true);
assert.equal(q.atFor(null), null);
assert.equal(q.atFor({ active: 5, playsets: [] }), null);

// late summary from an older epoch is dropped
assert.equal(q.responseIsCurrent(3, 4), false);
assert.equal(q.responseIsCurrent(4, 4), true);

// failed args save aborts launch; no args save, or a good one, launches
assert.equal(q.mayLaunch(false), false);
assert.equal(q.mayLaunch(true), true);
assert.equal(q.mayLaunch(null), true);

// Play anyway sends the dialog's opaque token verbatim
assert.equal(q.confirmExpect({ token: "a,b:c" }), "a,b:c");
assert.equal(q.confirmExpect(null), undefined);
console.log("queue.check ok");
