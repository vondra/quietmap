// The map's buttons and floating surfaces, one look each, on the type scale and the colour tokens of
// AGENTS.md's "UI style", so that the light and the dark scheme both hold.

/** An icon alone: a row's or a card's action. */
export const ICON_BUTTON = 'grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40'
/** A card's or a panel's action in words. */
export const BUTTON = 'inline-flex h-8 shrink-0 items-center justify-center gap-1.5 rounded-md border border-border bg-background px-3 text-sm font-medium text-foreground hover:bg-accent disabled:opacity-50'
/** The one action a panel is for. */
export const PRIMARY_BUTTON = 'inline-flex h-8 shrink-0 items-center justify-center gap-1.5 rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground hover:opacity-90 disabled:opacity-50'
/** A control floating on the map: the search field, the corner's card, the phone's account. */
export const FLOATING = 'rounded-lg border border-input bg-background text-foreground shadow-lg'
/** Words that act inline. */
export const TEXT_LINK = 'underline underline-offset-2 hover:text-foreground'
