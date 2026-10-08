// Splits RssAnon of a process by kind from its /proc/<pid>/smaps: node memsplit.mjs <pid>
//   [heap]            the main glibc malloc arena (brk): SQLite page caches, Buffers, strings that left V8, fragmentation
//   malloc arenas     64 MB-aligned anonymous mappings: the per-thread glibc arenas of the libuv / V8 worker threads
//   other anonymous   V8 heap pages (young and old generation, code), thread stacks, everything else mmap'd
// V8's own view (spaces, committed vs used) is in memreport.json (SIGURG, memreport.mjs).
import fs from "node:fs";
const pid = Number(process.argv[2]);
const lines = fs.readFileSync(`/proc/${pid}/smaps`, "utf8").split("\n");
let cur;
const maps = [];
for (const line of lines) {
  const head = line.match(/^([0-9a-f]+)-([0-9a-f]+) (\S+) \S+ \S+ \S+\s*(.*)$/);
  if (head) { cur = { start: BigInt("0x" + head[1]), end: BigInt("0x" + head[2]), perms: head[3], name: head[4], anon: 0 }; maps.push(cur); }
  else if (cur && line.startsWith("Anonymous:")) cur.anon = Number(line.match(/(\d+)/)[1]);
}
const kb = { heap: 0, arenas: 0, other: 0, stacks: 0 };
const arenaList = [];
for (const m of maps) {
  if (m.anon === 0) continue;
  if (m.name === "[heap]") kb.heap += m.anon;
  else if (m.name === "[stack]") kb.stacks += m.anon;
  else if (m.name === "" && Number(m.start % 0x4000000n) === 0 && m.end - m.start <= 0x4000000n && m.perms.startsWith("rw")) { kb.arenas += m.anon; arenaList.push(m.anon); }
  else kb.other += m.anon;
}
const mb = (x) => +(x / 1024).toFixed(1);
console.log(JSON.stringify({ pid, anonymousMb: mb(kb.heap + kb.arenas + kb.other + kb.stacks), mallocMainArenaMb: mb(kb.heap), mallocThreadArenasMb: mb(kb.arenas), arenaCount: arenaList.length, otherAnonymousMb_v8PagesStacksEtc: mb(kb.other + kb.stacks) }));
