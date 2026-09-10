/** Parse the complete 2021 MLIT road-traffic census for all 47 prefectures. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const CENSUS_FILES = [
  ['kasyo01.csv', '6a9d5b94bb771b80b7b4c298c825577a3b61d67d6c937c6a69f6caf54ed2b2e5'],
  ['kasyo02.csv', '34648f24e795a5ff5dabf089e99b1b6b81f0e935c54aaea7566defc3029549c3'],
  ['kasyo03.csv', 'a8af1ea2ca231b419007235b10ac4e02d9893b78d698009eda260376f98e1e0d'],
  ['kasyo04.csv', '1ae09c6ffcc65350ba2e0e7f9316ccae4d74a715ec53517747dc0b56ed2eed31'],
  ['kasyo05.csv', 'e9a81513033bd21a55cb800273624b5933867d7caa0971c352e1d738f296dcd4'],
  ['kasyo06.csv', 'b418e6e2f8ecb52c0d5e130150a1d93e93f4c40f90d127617671220200a439eb'],
  ['kasyo07.csv', 'ddbb26b9a20aaaf73dc4e5b0098c0db2b32fb06c123e5f16c81b077935cd2c92'],
  ['kasyo08.csv', 'eaeeddbf9f47608e350f70a0dff9a3a0f0af0b1f15941b7bf99b56a9cd8cd933'],
  ['kasyo09.csv', 'c33c2e85a38187567a4d0b4c82412a0ae065c12b670be48b74f3973991b970ee'],
  ['kasyo10.csv', 'e128e858e784aceb31a33153f3e530ae0bed3584a78db0acaa098b0ca1083694'],
  ['kasyo11.csv', '691fd97182425bd4397a8d8743b2b8192bd53154b5847886fd3b7b7ebd952afd'],
  ['kasyo12.csv', '5e10ffbcd7bc029d5eece06d6caa951a10a0ec7354ac30be045f1e21c7d92fd5'],
  ['kasyo13.csv', '37c41661cf92ee0f9964138694f43c5764f237dc061262f6457bbb6dd3e30c85'],
  ['kasyo14.csv', 'c50903ecd7daabca6cffa9a1f51905b8746d9e965dc8cc20704a6335664e638b'],
  ['kasyo15.csv', 'b8a6cae5f22b6fa66d6833746de3d22b550222b829c9d547ff13120e8f855553'],
  ['kasyo16.csv', '051adcf16741663bf37662b1fa7e42241439e918246676df8af4d9465c091f29'],
  ['kasyo17.csv', '22f93c3b69acc981fff66a2ccfb988abfb34e521d507709cdbcd229d9f3be33d'],
  ['kasyo18.csv', 'a0a5553b4aaf92dbc7626d1d3f8689ace4c8bc004c4502d2f0ebdd429881edf9'],
  ['kasyo19.csv', 'b0887b492a6d75c469ed2c20d24a71f10ca220f601e669bd42e7aa1449322ab8'],
  ['kasyo20.csv', 'be5c7adcf5964c3053f8226dbd7c64f93954d5eae9923865262a80358a911c35'],
  ['kasyo21.csv', '2869136e5c1f24f5d73ccb6ddb1c86a2c1fdea81a00db174ec02214d3163a469'],
  ['kasyo22.csv', 'b4f87df9772edd4f793fafc7c51aa268bb354dcb43099dddf368f56b1b3c9484'],
  ['kasyo23.csv', 'bda9df38b1e471c384748516281834913ab561563a093914e2b71be6bd853440'],
  ['kasyo24.csv', 'dae993f8104968bd04afff296856810d54ecddc05160e981392011c5787255be'],
  ['kasyo25.csv', 'fa6503bed633792b533140cfdfb28780069af0f3a6760ec8360e9132a0b41fdc'],
  ['kasyo26.csv', '4ff1c0cd7ed26e767e17dfc071280640ee438961067cc30110071684cee3fd50'],
  ['kasyo27.csv', 'e816251ff440c914ce5e094c4f4b6aa7ff900df6c429f3f66e1695ef79d4f516'],
  ['kasyo28.csv', '68c3e2ad78d6b89b92730ed24aad412e4342e9ae358f0436634683b2d5390ca2'],
  ['kasyo29.csv', 'e2d031397e1c12e51c6df69be016a4a3758446d46fafd54cab28cfddcc99242d'],
  ['kasyo30.csv', '7efb22c1791f8e758b84b6983c8d932d7059c788784b7df806f1f885eba6e179'],
  ['kasyo31.csv', 'cd6674f4def23c5ca843ca4a9aecf5fdac9f191718e6afbe32050082e022e063'],
  ['kasyo32.csv', 'eae1a6df149d24b6fad2d7a9d736b1617176a90e48135f76686dbb5921a03af1'],
  ['kasyo33.csv', '1fa1247266a59bec1323d7bb10f831c6082b8a06b10b09ce9e0366512ebfd21f'],
  ['kasyo34.csv', 'e210955433a1b93f417b19ee451fb0ed114176273266057ecc510509e432fc8c'],
  ['kasyo35.csv', 'c1285202d5a132ee69cd41e46c84a963d6e0878ca5da3657f0f5df2ce17308aa'],
  ['kasyo36.csv', 'd74b160faca692fa9a6c622e241605c94d26935c1ecc1cc3efb7d2b9d68cb043'],
  ['kasyo37.csv', '1ef5d30885c71db5999430aceadf60700c4150abe054edfe7f79dfc67ab8185d'],
  ['kasyo38.csv', '013eef69ca21230cde0844d2dce03a18a3dda789655eb2722fc110c632caac94'],
  ['kasyo39.csv', '06d1bc3c72f8408b64e4e9722efe88d196a798f3d6d3b9aa62050391c4a18831'],
  ['kasyo40.csv', 'b24d04c1b5e7807e5b76cc490214c68ec6cd93f03abc025ee46ee5fd47e6ab68'],
  ['kasyo41.csv', '7c1ff7ba6a02eea2aa79d32289ead1dbc511b28a1441f93abb95d99050c8e395'],
  ['kasyo42.csv', 'f01d8e1fc6ee08b9c48fcff643c320df0801e01cfe9bec37c1eaf8d721fea45b'],
  ['kasyo43.csv', 'b23be1b18127e79b807f2c8fba79f64ec85e2587bcab0ebec5e9b074be46af5f'],
  ['kasyo44.csv', '7e8d9a48755e689dc14d9a4ae0564f5a93cd7a164310277bd56688456317f68e'],
  ['kasyo45.csv', '7adac7535f3aa7750584a2b1be59973eeba25cf009b601f7e6c71632020f0229'],
  ['kasyo46.csv', '0cdeffb7e2a8078bb33608810df0ac4fc5e4671a47cdeafd62df16afacf1298b'],
  ['kasyo47.csv', '5f835319609e847b34ea30f9a826c95398ff1c47058a8d7c7d2033a71dd55c79'],
] as const
const COLUMN_ROAD_TYPE = 3
const COLUMN_ROUTE = 4
const COLUMN_NAME = 5
const COLUMN_SMALL = 59
const COLUMN_LARGE = 60
const TYPE_TO_CLASSES: Readonly<Record<string, readonly number[]>> = {
  '1': [0], '2': [0], '3': [1, 2], '4': [2, 3], '6': [4],
}

export interface JapaneseVehicleCounts {
  small: number
  large: number
}

export interface JapaneseRoadCensus {
  nationalByRef: ReadonlyMap<string, JapaneseVehicleCounts>
  expresswayByName: ReadonlyMap<string, JapaneseVehicleCounts>
  expresswayNames: readonly string[]
  classMedian: ReadonlyMap<number, JapaneseVehicleCounts>
  sourceRows: number
  admittedSections: number
  unsupportedTypeRows: number
  unavailableTrafficRows: number
  invalidRows: number
}

export function normalizeJapaneseRoadIdentity(value: string): string {
  return value.replace(/[０-９]/g,
    digit => String.fromCharCode(digit.charCodeAt(0) - 0xfee0)).replace(/\s+/g, '').trim()
}

export function leadingJapaneseRoadDigits(value: string): string {
  return normalizeJapaneseRoadIdentity(value).match(/\d+/)?.[0] ?? ''
}

function median(values: readonly number[]): number {
  if (values.length === 0) throw new Error('cannot take a median of no values')
  const sorted = [...values].sort((a, b) => a - b)
  const middle = sorted.length >> 1
  return sorted.length % 2 ? sorted[middle] : Math.round((sorted[middle - 1] + sorted[middle]) / 2)
}

const medianCounts = (values: readonly JapaneseVehicleCounts[]): JapaneseVehicleCounts => ({
  small: median(values.map(value => value.small)),
  large: median(values.map(value => value.large)),
})

function append<K>(map: Map<K, JapaneseVehicleCounts[]>, key: K, value: JapaneseVehicleCounts) {
  const values = map.get(key)
  if (values) values.push(value)
  else map.set(key, [value])
}

export function parseJapaneseRoadCensus(rawFiles: readonly Uint8Array[]): JapaneseRoadCensus {
  if (rawFiles.length !== 47) throw new Error(`Japanese census requires 47 prefectures, found ${rawFiles.length}`)
  const refRows = new Map<string, JapaneseVehicleCounts[]>()
  const nameRows = new Map<string, JapaneseVehicleCounts[]>()
  const classRows = new Map<number, JapaneseVehicleCounts[]>()
  let sourceRows = 0
  let admittedSections = 0
  let unsupportedTypeRows = 0
  let unavailableTrafficRows = 0
  let invalidRows = 0
  const unusableFiles: number[] = []
  for (const [fileIndex, raw] of rawFiles.entries()) {
    const lines = new TextDecoder('shift_jis').decode(raw).split(/\r?\n/)
    let fileSections = 0
    for (let lineIndex = 1; lineIndex < lines.length; lineIndex++) {
      if (!lines[lineIndex]) continue
      sourceRows++
      const fields = lines[lineIndex].split(',')
      if (fields.length <= COLUMN_LARGE) {
        invalidRows++
        continue
      }
      const classes = TYPE_TO_CLASSES[fields[COLUMN_ROAD_TYPE]]
      if (!classes) {
        unsupportedTypeRows++
        continue
      }
      const small = Number(fields[COLUMN_SMALL] || 0)
      const large = Number(fields[COLUMN_LARGE] || 0)
      if (!Number.isSafeInteger(small) || !Number.isSafeInteger(large) || small < 0 || large < 0) {
        invalidRows++
        continue
      }
      if (small + large === 0) {
        unavailableTrafficRows++
        continue
      }
      const counts = { small, large }
      admittedSections++
      fileSections++
      for (const roadClass of classes) append(classRows, roadClass, counts)
      if (fields[COLUMN_ROAD_TYPE] === '3') {
        const ref = leadingJapaneseRoadDigits(fields[COLUMN_ROUTE])
        if (ref) append(refRows, ref, counts)
      } else if (fields[COLUMN_ROAD_TYPE] === '1' || fields[COLUMN_ROAD_TYPE] === '2') {
        const name = normalizeJapaneseRoadIdentity(fields[COLUMN_NAME])
        if (name) append(nameRows, name, counts)
      }
    }
    if (fileSections === 0) unusableFiles.push(fileIndex + 1)
  }
  if (unusableFiles.length) {
    throw new Error(`Japanese census prefectures have no usable sections: ${unusableFiles.join(', ')}`)
  }
  const nationalByRef = new Map([...refRows].map(([key, values]) => [key, medianCounts(values)]))
  const expresswayByName = new Map([...nameRows].map(([key, values]) => [key, medianCounts(values)]))
  const classMedian = new Map([...classRows].map(([key, values]) => [key, medianCounts(values)]))
  for (const [link, parent] of [[10, 0], [11, 1], [12, 2]] as const) {
    const counts = classMedian.get(parent)
    if (counts) classMedian.set(link, counts)
  }
  return { nationalByRef, expresswayByName,
    expresswayNames: [...expresswayByName.keys()].sort((a, b) => b.length - a.length),
    classMedian, sourceRows, admittedSections, unsupportedTypeRows,
    unavailableTrafficRows, invalidRows }
}

export function loadJapaneseRoadCensus(options: RoadLoaderArguments): JapaneseRoadCensus {
  const files = CENSUS_FILES.map(([name, digest]) =>
    readPinnedRoadSource(options, `jp/${name}`, digest))
  return parseJapaneseRoadCensus(files)
}
