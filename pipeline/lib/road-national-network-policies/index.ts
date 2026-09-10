/** Registry of pinned road-network classification proxy policies. */

import { policy as br } from './br.js'
import { policy as bo } from './bo.js'
import { policy as cn } from './cn.js'
import { policy as ec } from './ec.js'
import { policy as india } from './in.js'
import { policy as ph } from './ph.js'
import { policy as py } from './py.js'
import { policy as ve } from './ve.js'
import type { NationalRoadLinePolicy } from './types.js'

export type { NationalRoadLinePolicy } from './types.js'

const POLICIES = [bo, br, cn, ec, india, ph, py, ve]
export const NATIONAL_ROAD_NETWORK_POLICIES: ReadonlyMap<string, NationalRoadLinePolicy> =
  new Map(POLICIES.map(policy => [policy.country, policy]))
