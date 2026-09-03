import type { FileSearchHit } from "../bindings";

export interface FileSearchGroup {
  path: string;
  hits: FileSearchHit[];
}

/**
 * Group search hits by file path, preserving relative hit order within each file.
 */
export function groupSearchHits(hits: FileSearchHit[]): FileSearchGroup[] {
  const groups: FileSearchGroup[] = [];
  const byPath = new Map<string, FileSearchGroup>();

  for (const hit of hits) {
    let group = byPath.get(hit.path);
    if (!group) {
      group = { path: hit.path, hits: [] };
      byPath.set(hit.path, group);
      groups.push(group);
    }
    group.hits.push(hit);
  }

  return groups;
}
