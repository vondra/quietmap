/**
 * Binary min-heap for Dijkstra's algorithm — extracted 2026-07-16 from
 * `enrich-roads-service-tree.ts` and `rail-graph-metrics.ts`, which carried
 * byte-identical copies of the same (dist, node) priority queue.
 */

export class MinHeap {
  private data: { dist: number; node: number }[] = []

  push(dist: number, node: number) {
    this.data.push({ dist, node })
    let i = this.data.length - 1
    while (i > 0) {
      const p = (i - 1) >> 1
      if (this.data[p].dist <= this.data[i].dist) break
      ;[this.data[p], this.data[i]] = [this.data[i], this.data[p]]
      i = p
    }
  }

  pop(): { dist: number; node: number } {
    const top = this.data[0]
    const last = this.data.pop()!
    if (this.data.length > 0) {
      this.data[0] = last
      let i = 0
      while (true) {
        let smallest = i
        const l = 2 * i + 1, r = 2 * i + 2
        if (l < this.data.length && this.data[l].dist < this.data[smallest].dist) smallest = l
        if (r < this.data.length && this.data[r].dist < this.data[smallest].dist) smallest = r
        if (smallest === i) break
        ;[this.data[i], this.data[smallest]] = [this.data[smallest], this.data[i]]
        i = smallest
      }
    }
    return top
  }

  get size() { return this.data.length }
}

