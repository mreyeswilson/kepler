import { readFileSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";

/**
 * Current git branch of `cwd`, read from `.git/HEAD` (no process is spawned). Handles linked
 * worktrees (`.git` file with `gitdir:`). Detached HEAD, a missing repository or any error gives
 * `undefined`: the branch is an optional hint (ADR-0021), never required.
 */
export function currentBranch(cwd: string): string | undefined {
  try {
    let dir = resolve(cwd);
    for (let i = 0; i < 12; i++) {
      const dotGit = join(dir, ".git");
      let st;
      try {
        st = statSync(dotGit);
      } catch {
        st = undefined;
      }
      if (st !== undefined) {
        let gitDir = dotGit;
        if (st.isFile()) {
          const m = /^gitdir:\s*(.+)$/m.exec(readFileSync(dotGit, "utf8"));
          if (m === null) return undefined;
          const g = (m[1] ?? "").trim();
          gitDir = isAbsolute(g) ? g : resolve(dir, g);
        }
        const head = readFileSync(join(gitDir, "HEAD"), "utf8").trim();
        const r = /^ref:\s*refs\/heads\/(.+)$/.exec(head);
        const name = r?.[1]?.trim();
        if (name === undefined || name.length === 0 || name.length > 128 || /[\0\r\n]/.test(name)) return undefined;
        return name;
      }
      const parent = dirname(dir);
      if (parent === dir) return undefined;
      dir = parent;
    }
  } catch {
    /* optional hint */
  }
  return undefined;
}
