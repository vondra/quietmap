/** Registry of national OSM-class traffic policies that share one writer. */

import { policy as cd } from './cd.js'
import { policy as dz } from './dz.js'
import { policy as eg } from './eg.js'
import { policy as et } from './et.js'
import { policy as iq } from './iq.js'
import { policy as ir } from './ir.js'
import { policy as ke } from './ke.js'
import { policy as kz } from './kz.js'
import { policy as ma } from './ma.js'
import { policy as ng } from './ng.js'
import { policy as ru } from './ru.js'
import { policy as sd } from './sd.js'
import { policy as tr } from './tr.js'
import { policy as tz } from './tz.js'
import { policy as ua } from './ua.js'
import { policy as uz } from './uz.js'
import type { NationalRoadPolicy } from './types.js'

export type { NationalRoadPolicy, PolicyRoadTraffic } from './types.js'

const POLICIES = [cd, dz, eg, et, iq, ir, ke, kz, ma, ng, ru, sd, tr, tz, ua, uz]

export const NATIONAL_ROAD_POLICIES: ReadonlyMap<string, NationalRoadPolicy> =
  new Map(POLICIES.map(policy => [policy.country, policy]))
