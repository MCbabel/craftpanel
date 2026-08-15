import { describe, expect, it } from 'vitest'

import { actionsColumnWidth, ICON_BUTTON_REM, ICON_LABEL_BUTTON_REM } from './table-widths'

const ROOT_FONT_PX = 16
const ICON_BUTTON_PX = 36
const ICON_LABEL_BUTTON_PX = 40
const CELL_PADDING_PX = 16
const BUTTON_GAP_PX = 8
const FOCUS_RING_PX = 4

function px(width: string): number {
	const rem = Number(width.replace(/rem$/, ''))
	expect(Number.isFinite(rem)).toBe(true)
	return rem * ROOT_FONT_PX
}

describe('actionsColumnWidth', () => {
	it('shows a single icon button whole, with its padding and focus ring', () => {
		expect(px(actionsColumnWidth([ICON_BUTTON_REM]))).toBe(
			ICON_BUTTON_PX + CELL_PADDING_PX + FOCUS_RING_PX,
		)
	})

	it('counts in the wider button whose label is read out only', () => {
		expect(px(actionsColumnWidth([ICON_LABEL_BUTTON_REM]))).toBe(
			ICON_LABEL_BUTTON_PX + CELL_PADDING_PX + FOCUS_RING_PX,
		)
	})

	it('counts three gaps between four buttons', () => {
		expect(px(actionsColumnWidth(Array.from({ length: 4 }, () => ICON_BUTTON_REM)))).toBe(
			4 * ICON_BUTTON_PX + 3 * BUTTON_GAP_PX + CELL_PADDING_PX + FOCUS_RING_PX,
		)
	})

	it('leaves every row of buttons more room than it needs', () => {
		for (const count of [1, 2, 3, 4, 5]) {
			const buttons = Array.from({ length: count }, () => ICON_BUTTON_REM)
			const needed = count * ICON_BUTTON_PX + (count - 1) * BUTTON_GAP_PX + CELL_PADDING_PX
			expect(px(actionsColumnWidth(buttons))).toBeGreaterThan(needed)
		}
	})
})
