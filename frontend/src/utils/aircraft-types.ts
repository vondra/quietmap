// ICAO aircraft type designators (Doc 8643) in words, for the popup's loudest flights: the types
// heard most in Europe, light aircraft included. Any other designator is shown as it is.

const AIRCRAFT_TYPE_NAMES: Record<string, string> = {
  // Boeing 737
  B733: 'Boeing 737-300', B734: 'Boeing 737-400', B735: 'Boeing 737-500', B736: 'Boeing 737-600',
  B737: 'Boeing 737-700', B738: 'Boeing 737-800', B739: 'Boeing 737-900',
  B37M: 'Boeing 737 MAX 7', B38M: 'Boeing 737 MAX 8', B39M: 'Boeing 737 MAX 9',
  // Airbus A220 and A320 families
  BCS1: 'Airbus A220-100', BCS3: 'Airbus A220-300',
  A319: 'Airbus A319', A320: 'Airbus A320', A321: 'Airbus A321',
  A19N: 'Airbus A319neo', A20N: 'Airbus A320neo', A21N: 'Airbus A321neo',
  // Boeing 717, 747, 757, 767, 777, 787
  B712: 'Boeing 717-200',
  B741: 'Boeing 747-100', B742: 'Boeing 747-200', B744: 'Boeing 747-400', B748: 'Boeing 747-8',
  B752: 'Boeing 757-200', B753: 'Boeing 757-300', B763: 'Boeing 767-300', B764: 'Boeing 767-400',
  B772: 'Boeing 777-200', B773: 'Boeing 777-300', B77L: 'Boeing 777-200LR', B77W: 'Boeing 777-300ER',
  B77F: 'Boeing 777F',
  B788: 'Boeing 787-8', B789: 'Boeing 787-9', B78X: 'Boeing 787-10',
  // Airbus A300 to A380
  A306: 'Airbus A300-600', A310: 'Airbus A310',
  A332: 'Airbus A330-200', A333: 'Airbus A330-300', A338: 'Airbus A330-800neo', A339: 'Airbus A330-900neo',
  A342: 'Airbus A340-200', A343: 'Airbus A340-300', A346: 'Airbus A340-600',
  A359: 'Airbus A350-900', A35K: 'Airbus A350-1000', A388: 'Airbus A380-800',
  // Other airliners and freighters
  MD11: 'McDonnell Douglas MD-11', DC10: 'McDonnell Douglas DC-10', L101: 'Lockheed L-1011 TriStar',
  IL76: 'Ilyushin Il-76',
  // Embraer (both 175 wings under one name: the designator is in the tooltip)
  E135: 'Embraer ERJ-135', E145: 'Embraer ERJ-145',
  E170: 'Embraer 170', E75L: 'Embraer 175', E75S: 'Embraer 175', E190: 'Embraer 190', E195: 'Embraer 195',
  E290: 'Embraer E190-E2', E295: 'Embraer E195-E2',
  E55P: 'Embraer Phenom 300', E545: 'Embraer Legacy 450', E550: 'Embraer Legacy 500',
  E35L: 'Embraer Legacy 600/650',
  // Bombardier, Learjet, Gulfstream, Dassault, Hawker
  CRJ2: 'Bombardier CRJ-200', CRJ7: 'Bombardier CRJ-700', CRJ9: 'Bombardier CRJ-900',
  CL30: 'Bombardier Challenger 300', CL35: 'Bombardier Challenger 350',
  CL60: 'Bombardier Challenger 600', CL64: 'Bombardier Challenger 650',
  GLEX: 'Bombardier Global Express', GL5T: 'Bombardier Global 5000', GL7T: 'Bombardier Global 7500',
  LJ45: 'Learjet 45', LJ60: 'Learjet 60',
  GLF4: 'Gulfstream G450', GLF5: 'Gulfstream G550', GLF6: 'Gulfstream G650', GLF7: 'Gulfstream G700',
  F2TH: 'Dassault Falcon 2000', F900: 'Dassault Falcon 900', H25B: 'Hawker 800XP',
  // Cessna
  C150: 'Cessna 150', C152: 'Cessna 152', C170: 'Cessna 170', C172: 'Cessna 172 Skyhawk',
  C177: 'Cessna 177 Cardinal', C180: 'Cessna 180', C182: 'Cessna 182 Skylane', C185: 'Cessna 185',
  C208: 'Cessna 208 Caravan', C310: 'Cessna 310', C402: 'Cessna 402',
  C510: 'Cessna Citation Mustang', C525: 'Cessna CitationJet', C25A: 'Cessna Citation CJ2',
  C25B: 'Cessna Citation CJ3', C25C: 'Cessna Citation CJ4', C550: 'Cessna Citation II',
  C560: 'Cessna Citation V', C56X: 'Cessna Citation Excel/XLS', C68A: 'Cessna Citation Latitude',
  C700: 'Cessna Citation Longitude', C750: 'Cessna Citation X',
  // Piper
  P28A: 'Piper PA-28 Cherokee', P28B: 'Piper PA-28 Dakota', P28R: 'Piper PA-28R Arrow',
  P32R: 'Piper PA-32R Saratoga', PA24: 'Piper PA-24 Comanche', PA31: 'Piper PA-31 Navajo',
  PA46: 'Piper PA-46 Malibu',
  // Beechcraft
  BE20: 'Beech King Air 200', BE33: 'Beech 33 Bonanza', BE35: 'Beech V35 Bonanza', BE40: 'Beechjet 400',
  BE58: 'Beech Baron 58', BE95: 'Beech 95 Travel Air', BE9L: 'Beech 90 King Air', B190: 'Beech 1900',
  B350: 'Beech King Air 350',
  // Turboprops and light aircraft
  AT72: 'ATR 72', AT75: 'ATR 72-500', AT76: 'ATR 72-600',
  DH8A: 'DHC-8-100 Dash 8', DH8C: 'DHC-8-300', DH8D: 'DHC-8-400',
  PC12: 'Pilatus PC-12', TBM7: 'Daher TBM-700', SF34: 'Saab 340', SF50: 'Cirrus SF50 Vision Jet',
  SR20: 'Cirrus SR20', SR22: 'Cirrus SR22', S22T: 'Cirrus SR22T',
  RV4: "Van's RV-4", RV6: "Van's RV-6", RV9: "Van's RV-9", RV10: "Van's RV-10", RV12: "Van's RV-12",
  // Light aircraft of European skies
  C140: 'Cessna 140', C206: 'Cessna 206 Stationair', C210: 'Cessna 210 Centurion',
  C72R: 'Cessna 172RG Cutlass', C82R: 'Cessna 182RG Skylane RG', C77R: 'Cessna 177RG Cardinal RG',
  PA18: 'Piper PA-18 Super Cub', PA32: 'Piper PA-32 Cherokee Six', PA34: 'Piper PA-34 Seneca',
  PA38: 'Piper PA-38 Tomahawk', PA44: 'Piper PA-44 Seminole',
  DA20: 'Diamond DA20 Katana', DA40: 'Diamond DA40 Diamond Star', DA42: 'Diamond DA42 Twin Star',
  DA62: 'Diamond DA62', G115: 'Grob G115', G120: 'Grob G120',
  TB10: 'Socata TB-10 Tobago', TB20: 'Socata TB-20 Trinidad', DR40: 'Robin DR400',
  M20P: 'Mooney M20', M20T: 'Mooney M20 (turbo)', E300: 'Extra 300', XA42: 'XtremeAir XA42',
  Z42: 'Zlin Z-42', Z43: 'Zlin Z-43', Z142: 'Zlin Z-142', Z242: 'Zlin Z-242',
  L200: 'Let L-200 Morava', L410: 'Let L-410 Turbolet', AN2: 'Antonov An-2',
  VUT1: 'Evektor VUT100 Cobra', WT9: 'Aerospool WT9 Dynamic', P208: 'Tecnam P2008',
  FK14: 'FK Lightplanes FK14 Polaris', GLID: 'glider', BALL: 'balloon',
  // Helicopters
  AS50: 'Aérospatiale AS350 Écureuil', AS55: 'Aérospatiale AS355 Écureuil 2',
  AS65: 'Aérospatiale AS365 Dauphin',
  EC30: 'Eurocopter EC130', EC35: 'Eurocopter EC135', EC45: 'Eurocopter EC145',
  EC55: 'Eurocopter EC155', EC75: 'Eurocopter EC175', H160: 'Airbus H160',
  A109: 'Leonardo A109', A119: 'Leonardo A119', A139: 'Leonardo AW139', A169: 'Leonardo AW169',
  A189: 'Leonardo AW189',
  B06: 'Bell 206 JetRanger', B407: 'Bell 407', B412: 'Bell 412', B427: 'Bell 427', B429: 'Bell 429',
  B505: 'Bell 505', EC20: 'Eurocopter EC120', EN48: 'Enstrom 480', H500: 'MD 500',
  R22: 'Robinson R22', R44: 'Robinson R44', R66: 'Robinson R66', S76: 'Sikorsky S-76', S92: 'Sikorsky S-92',
}

/** The type in words, else its designator as it is. */
export function aircraftTypeName(designator: string): string {
  return Object.hasOwn(AIRCRAFT_TYPE_NAMES, designator) ? AIRCRAFT_TYPE_NAMES[designator] : designator
}
