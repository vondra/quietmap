// The optional sign-in in a real browser, the server mocked: a signed-out visitor finds it at the top
// right and under the history of places opened, and asks for a link; the link's page signs in only at a press and
// adds the browser's places only at the visitor's word, keeping them in the browser too; a signed-in
// visitor's names come from the account and go to it, the browser's own left as they were until
// signing out at the top right.
import { expect, test, type Page, type Route } from '@playwright/test'
import { PHONE, POINT, installHermeticMap, openPoint, renameOpenPlace } from './support'

test.use(PHONE)

const ACCOUNT = 'owner@example.org'
const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) })

/** Seeds this browser's own places once, before the first document of the test. */
async function seedBrowserPlaces(page: Page, titles: object[], recent: object[] = []) {
  await page.addInitScript(([titles, recent]) => {
    if (sessionStorage.getItem('seeded')) return
    localStorage.setItem('quietmap.place-titles', JSON.stringify(titles))
    localStorage.setItem('quietmap.recent-places', JSON.stringify(recent))
    sessionStorage.setItem('seeded', '1')
  }, [titles, recent])
}

const storedTitles = (page: Page) => page.evaluate(() => JSON.parse(localStorage.getItem('quietmap.place-titles') ?? '[]'))

test('a signed-out visitor finds the sign-in at the top right and under the places opened, and asks for a link', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.route('**/api/me', route => json(route, { error: 'Not signed in' }, 401))
  const asked: unknown[] = []
  await page.route('**/api/auth/email', route => {
    asked.push(route.request().postDataJSON())
    return route.fulfill({ status: 204 })
  })
  await openPoint(page)
  const account = page.getByTestId('account')
  await expect(account.getByRole('button', { name: 'Sign in' })).toBeVisible()
  await expect(account.getByRole('textbox', { name: 'E-mail' })).toHaveCount(0)
  await page.getByRole('combobox').tap()
  await page.getByRole('button', { name: 'Sign in to keep these places on all your devices' }).tap()
  await account.getByRole('textbox', { name: 'E-mail' }).fill(ACCOUNT)
  await account.getByRole('button', { name: 'Send me a link' }).tap()
  await expect(account.getByTestId('sign-in-sent')).toContainText(ACCOUNT)
  expect(asked).toEqual([{ email: ACCOUNT }])
  // A tap elsewhere closes it; the top right opens it again.
  await page.getByTestId('mobile-detail-sheet').getByRole('button', { name: 'Close' }).tap()
  await expect(page.getByTestId('mobile-detail-sheet')).toHaveCount(0)
  await expect(account.getByTestId('sign-in-sent')).toHaveCount(0)
  await account.getByRole('button', { name: 'Sign in' }).tap()
  await expect(account.getByRole('textbox', { name: 'E-mail' })).toBeVisible()
})

test("a signed-in visitor's names come from the account and go to it; signed out, the browser's are back", async ({ page }) => {
  await installHermeticMap(page, POINT)
  await seedBrowserPlaces(page, [{ lat: POINT.lat, lng: POINT.lng, title: 'Browser name' }])
  let signedIn = true
  await page.route('**/api/me', route => (signedIn
    ? json(route, { email: ACCOUNT, titles: [{ lat: POINT.lat, lng: POINT.lng, title: 'Account name', updatedAt: 5 }], recent: [] })
    : json(route, { error: 'Not signed in' }, 401)))
  const sent: { lat: number, lng: number, title: string, updatedAt: number }[] = []
  await page.route('**/api/places/title', route => {
    const name = route.request().postDataJSON()
    expect(route.request().headers()['x-quietmap-account']).toBe(ACCOUNT)
    sent.push(name)
    return json(route, { titles: [{ lat: POINT.lat, lng: POINT.lng, title: name.title, updatedAt: name.updatedAt }] })
  })
  await page.route('**/api/places/recent', route => route.fulfill({ status: 204 }))
  await page.route('**/api/auth/signout', route => {
    signedIn = false
    return route.fulfill({ status: 204 })
  })

  const row = await openPoint(page)
  await expect(row).toContainText('Account name')
  await page.getByRole('combobox').tap()
  await expect(page.getByRole('option', { name: /Account name/ })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Sign in to keep these places on all your devices' })).toHaveCount(0)
  await page.keyboard.press('Escape')
  await renameOpenPlace(page, 'Renamed')
  await expect.poll(() => sent.map(name => name.title)).toEqual(['Renamed'])
  // The tapped point, which is POINT up to the map's float arithmetic.
  expect(Math.abs(sent[0].lat - POINT.lat) + Math.abs(sent[0].lng - POINT.lng)).toBeLessThan(1e-6)
  expect(sent[0].updatedAt).toBeGreaterThan(Date.parse('2026-01-01'))
  expect((await storedTitles(page)).map((name: { title: string }) => name.title)).toEqual(['Browser name'])

  const account = page.getByTestId('account')
  await account.getByRole('button', { name: `Account: ${ACCOUNT}` }).tap()
  await account.getByRole('button', { name: 'Sign out' }).tap()
  await expect(account.getByRole('button', { name: 'Sign in' })).toBeVisible()
  await expect(await openPoint(page)).toContainText('Browser name')
})

test("the link's page signs in only at a press and adds the browser's places only at the visitor's word", async ({ page }) => {
  await installHermeticMap(page, POINT)
  await seedBrowserPlaces(page,
    [{ lat: 50, lng: 14, title: 'Cabin', updatedAt: 3 }],
    [{ lat: 50, lng: 14, place: 'Bedřichov, Czechia', sone: 0.3, lden: 31 }])
  const used: unknown[] = []
  const added: unknown[] = []
  await page.route('**/api/auth/link', route => json(route, { email: ACCOUNT }))
  await page.route('**/api/auth/link/use', route => {
    used.push(route.request().postDataJSON())
    return route.fulfill({ status: 204 })
  })
  await page.route('**/api/places/import', route => {
    added.push(route.request().postDataJSON())
    return json(route, added.at(-1))
  })
  await page.route('**/api/me', route => json(route, { email: ACCOUNT, titles: [], recent: [] }))

  await page.goto('/signin#a-token')
  await expect(page.getByRole('heading', { name: `Sign in as ${ACCOUNT}?` })).toBeVisible()
  // The token has left the address bar, and nothing is used before the press.
  expect(new URL(page.url()).hash).toBe('')
  expect(used).toEqual([])
  await page.getByRole('button', { name: 'Sign in' }).tap()
  await expect(page.getByText('This browser keeps 1 named place and 1 recent place.')).toBeVisible()
  expect(used).toEqual([{ token: 'a-token' }])
  await page.getByRole('button', { name: 'Add them' }).tap()
  await page.waitForURL(url => url.pathname === '/')
  expect(added).toEqual([{
    titles: [{ lat: 50, lng: 14, title: 'Cabin', updatedAt: 3 }],
    // Kept before the history counted: opened once, at no known time.
    recent: [{ lat: 50, lng: 14, place: 'Bedřichov, Czechia', sone: 0.3, lden: 31, count: 1, openedAt: 0 }],
  }])
  // Added to the account, they stay in this browser's own store as well, for when it signs out.
  expect(await storedTitles(page)).toEqual([{ lat: 50, lng: 14, title: 'Cabin', updatedAt: 3 }])
})

test('an expired link says so and signs nobody in', async ({ page }) => {
  await page.route('**/api/auth/link', route => json(route, { error: 'This link has expired or was used' }, 404))
  await page.goto('/signin#old-token')
  await expect(page.getByRole('heading', { name: 'This link has expired or was used already.' })).toBeVisible()
})
