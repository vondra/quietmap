// The shpjs package ships no type declarations; the two SHP enrichers
// (enrich-roads-pl.ts, enrich-industrial-se.ts) await one call and read the
// GeoJSON it returns as untyped data.
declare module 'shpjs' {
  export default function shp(input: Buffer | ArrayBuffer | string): Promise<any>
}
