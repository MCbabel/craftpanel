import { defineMessage, useVIntl } from '@modrinth/ui'

const messages = [
	defineMessage({
		id: 'format.decimal-bytes.0',
		defaultMessage: '{count, plural, one {# byte} other {# bytes}}',
	}),
	defineMessage({ id: 'format.decimal-bytes.1', defaultMessage: '{count, number} kB' }),
	defineMessage({ id: 'format.decimal-bytes.2', defaultMessage: '{count, number} MB' }),
	defineMessage({ id: 'format.decimal-bytes.3', defaultMessage: '{count, number} GB' }),
	defineMessage({ id: 'format.decimal-bytes.4', defaultMessage: '{count, number} TB' }),
]

function unitOf(bytes: number): number {
	let exponent = 0
	while (exponent < messages.length - 1 && bytes >= Math.pow(1000, exponent + 1)) exponent += 1
	return exponent
}

export function useFormatDecimalBytes() {
	const { formatMessage } = useVIntl()

	function format(bytes: number, decimals = 2): string {
		if (bytes === 0) return formatMessage(messages[0], { count: 0 })

		const exponent = unitOf(bytes)
		return formatMessage(messages[exponent], {
			count: (bytes / Math.pow(1000, exponent)).toFixed(decimals),
		})
	}

	return format
}
