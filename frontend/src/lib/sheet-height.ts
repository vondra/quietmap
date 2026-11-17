// The open phone sheets' height, the tallest, as --sheet-height on the root: the map's credit stands on it.
const heights = new Map<Element, number>()

function publish() {
  document.documentElement.style.setProperty('--sheet-height', `${Math.max(0, ...heights.values())}px`)
}

const observer = new ResizeObserver(entries => {
  for (const entry of entries) heights.set(entry.target, entry.borderBoxSize[0].blockSize)
  publish()
})

/** A phone sheet's callback ref: its height counts while it is mounted (React runs the cleanup). */
export function sheetHeightRef(element: HTMLElement | null) {
  if (!element) return
  observer.observe(element)
  return () => {
    observer.unobserve(element)
    heights.delete(element)
    publish()
  }
}
