/** First road chain step: make a finalized generation enrichable again; raw squares are untouched. */

import { relative, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { restoreRoadParentsForEnrichment } from './lib/road-parent.js'
import { runSquareSteps } from './lib/square-pool.js'
import { SourceTransportTopology } from './lib/transport-topology.js'

async function main(): Promise<void> {
  let topology: SourceTransportTopology | undefined
  await runSquareSteps('usage: restore-road-parents.ts --prepared-dir PREPARED_YEAR_DIR', async directory => {
    const prepared = resolve(directory, '../../..')
    return restoreRoadParentsForEnrichment(resolve(directory, 'roads.arrow'), relative(prepared, directory),
      topology ??= new SourceTransportTopology(prepared, 'roads'))
  })
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => { console.error(error); process.exitCode = 1 })
}
