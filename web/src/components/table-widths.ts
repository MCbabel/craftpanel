export const ICON_BUTTON_REM = 2.25
export const ICON_LABEL_BUTTON_REM = 2.5

const CELL_PADDING_REM = 1
const BUTTON_GAP_REM = 0.5
const FOCUS_RING_REM = 0.25

export function actionsColumnWidth(buttonsRem: number[]): string {
	const buttons = buttonsRem.reduce((sum, width) => sum + width, 0)
	const gaps = BUTTON_GAP_REM * Math.max(0, buttonsRem.length - 1)
	return `${buttons + gaps + CELL_PADDING_REM + FOCUS_RING_REM}rem`
}
