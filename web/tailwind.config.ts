import preset from '@modrinth/tooling-config/tailwind/tailwind-preset.ts'
import type { Config } from 'tailwindcss'

const config: Config = {
	content: [
		'./index.html',
		'./src/**/*.{js,ts,vue}',
		'../vendor/modrinth/ui/src/**/*.{js,ts,vue}',
		'../vendor/modrinth/ui/index.ts',
	],
	presets: [preset],
}

export default config
