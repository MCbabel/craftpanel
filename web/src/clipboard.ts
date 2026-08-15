let reportFailure: (() => void) | null = null

export function setCopyFailureHandler(handler: (() => void) | null): void {
	reportFailure = handler
}

function canFocus(node: unknown): node is { focus: () => void } {
	return typeof (node as { focus?: unknown } | null)?.focus === 'function'
}

export function copyBySelection(text: string, page: Document = document): boolean {
	const field = page.createElement('textarea')
	field.value = text
	field.setAttribute('readonly', '')
	field.style.position = 'fixed'
	field.style.top = '-1000px'
	field.style.opacity = '0'

	const focused = page.activeElement
	const selection = page.getSelection()
	const previous = selection !== null && selection.rangeCount > 0 ? selection.getRangeAt(0) : null

	page.body.append(field)
	field.select()

	let copied = false
	try {
		copied = page.execCommand('copy')
	} catch {
		copied = false
	}

	field.remove()
	if (selection !== null && previous !== null) {
		selection.removeAllRanges()
		selection.addRange(previous)
	}
	if (canFocus(focused)) focused.focus()

	return copied
}

export function installClipboardFallback(
	host: { clipboard?: unknown } = navigator,
	copy: (text: string) => boolean = copyBySelection,
): boolean {
	if (host.clipboard) return false

	const clipboard = {
		writeText(text: string): Promise<void> {
			if (copy(text)) return Promise.resolve()
			reportFailure?.()
			return Promise.reject(new Error('The browser refused to copy'))
		},
	}
	Object.defineProperty(host, 'clipboard', { configurable: true, value: clipboard })
	return true
}
