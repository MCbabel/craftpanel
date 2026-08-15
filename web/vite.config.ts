import vue from '@vitejs/plugin-vue'
import { resolve } from 'node:path'
import { defineConfig } from 'vite'
import svgLoader from 'vite-svg-loader'

const root = resolve(__dirname)
const vendor = resolve(root, '../vendor/modrinth')

export default defineConfig({
	plugins: [
		vue(),
		// SVGO drops the viewBox by default when an svg carries width and height.
		// Every icon then draws its 24-unit artwork raw into a 20px box and loses
		// whatever sits past the edge — the boxes and the database icon were
		// visibly cut off at the bottom. Both Modrinth apps set this; so must we.
		svgLoader({
			svgoConfig: {
				plugins: [
					{
						name: 'preset-default',
						params: {
							overrides: {
								removeViewBox: false,
								cleanupIds: { minify: false },
							},
						},
					},
				],
			},
		}),
	],
	css: {
		preprocessorOptions: {
			scss: {
				silenceDeprecations: ['import', 'global-builtin', 'legacy-js-api'],
			},
		},
	},
	resolve: {
		alias: [
			{ find: '@modrinth/api-client', replacement: resolve(vendor, 'api-client/src/index.ts') },
			{ find: '@', replacement: resolve(root, 'src') },
		],
	},
	optimizeDeps: {
		include: ['vue-router', 'floating-vue', '@floating-ui/dom'],
	},
	server: {
		host: '127.0.0.1',
		port: 5173,
		proxy: {
			'/api': { target: 'http://127.0.0.1:8080', changeOrigin: true, ws: true },
		},
	},
	build: {
		outDir: 'dist',
		emptyOutDir: true,
		chunkSizeWarningLimit: 2000,
	},
})
