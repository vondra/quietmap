/// <reference types="vite/client" />

/** The About pages by path under /about: their titles, and those their parent does not list. */
declare module 'virtual:about-index' {
  const index: Readonly<Record<string, { title: string }>>
  export default index
}
