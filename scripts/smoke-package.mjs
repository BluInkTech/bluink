import { spawn } from 'node:child_process'
import { cp, mkdtemp, readFile, rm, stat } from 'node:fs/promises'
import { createServer } from 'node:net'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createRequire } from 'node:module'
import { json, npm, root, run, targets } from './release-utils.mjs'

async function checkServer(cli, project) {
	const reservation = createServer()
	await new Promise((resolve, reject) => {
		reservation.once('error', reject)
		reservation.listen(0, '127.0.0.1', resolve)
	})
	const port = reservation.address().port
	await new Promise((resolve) => reservation.close(resolve))
	const child = spawn(
		process.execPath,
		[cli, 'server', '--project', project, '--port', String(port)],
		{ cwd: path.dirname(project), stdio: ['ignore', 'pipe', 'pipe'] },
	)
	let output = ''
	let failure
	child.on('error', (error) => {
		failure = error
	})
	child.stdout.on('data', (chunk) => {
		output += chunk
	})
	child.stderr.on('data', (chunk) => {
		output += chunk
	})
	const closed = new Promise((resolve) => child.once('close', resolve))
	try {
		for (let attempt = 0; attempt < 100; attempt++) {
			if (failure || child.exitCode !== null)
				throw new Error(`Packaged server failed: ${failure?.message ?? output}`)
			const response = await fetch(`http://127.0.0.1:${port}/`, {
				signal: AbortSignal.timeout(1000),
			}).catch(() => null)
			if (response?.ok) {
				const html = await response.text()
				if (!html.includes('<html') || !html.includes('/assets/'))
					throw new Error('Packaged server did not serve the built UI.')
				const assets = [
					...html.matchAll(/<(?:script|link)\b[^>]*(?:src|href)="([^"]+)"/g),
				]
					.map((match) => match[1])
					.filter((url) => /\.(?:css|js)(?:\?|$)/.test(url))
				if (
					!assets.some((url) => url.endsWith('.css')) ||
					!assets.some((url) => url.endsWith('.js'))
				)
					throw new Error(
						'Packaged HTML does not reference CSS and JavaScript bundles.',
					)
				for (const asset of assets) {
					const resource = await fetch(
						new URL(asset, `http://127.0.0.1:${port}/`),
					)
					const expectedType = asset.split('?')[0].endsWith('.css')
						? 'text/css'
						: 'text/javascript'
					if (
						!resource.ok ||
						resource.headers.get('content-type')?.split(';')[0] !== expectedType
					)
						throw new Error(
							`Packaged frontend asset ${asset} returned HTTP ${resource.status} with ${resource.headers.get('content-type')}.`,
						)
					if (!(await resource.arrayBuffer()).byteLength)
						throw new Error(`Packaged frontend asset is empty: ${asset}`)
				}
				for (const forbidden of [
					'/src/main.tsx',
					'/@vite/client',
					'/.env',
					'/package.json',
					...assets.map((asset) => `${asset}.map`),
				]) {
					const response = await fetch(
						new URL(forbidden, `http://127.0.0.1:${port}/`),
					)
					if (response.status !== 404)
						throw new Error(
							`Production source/debug path is accessible: ${forbidden}`,
						)
				}
				// Fonts are loaded by CSS at runtime and do not appear in index.html.
				const css = await fetch(
					new URL('/fonts/inter.css', `http://127.0.0.1:${port}/`),
				)
				if (!css.ok || css.headers.get('content-type') !== 'text/css')
					throw new Error(
						'Packaged font stylesheet is missing or has an incorrect MIME type.',
					)
				const fontPath = (await css.text()).match(
					/url\('\.\/([^']+\.woff2)'\)/,
				)?.[1]
				if (!fontPath)
					throw new Error('Packaged font stylesheet has no font asset.')
				const fontUrl = `/fonts/${fontPath}`
				const font = await fetch(new URL(fontUrl, `http://127.0.0.1:${port}/`))
				if (
					!font.ok ||
					font.headers.get('content-type') !== 'font/woff2' ||
					!(await font.arrayBuffer()).byteLength
				)
					throw new Error(
						'Packaged UI font is missing or served with an incorrect MIME type.',
					)
				return
			}
			await new Promise((resolve) => setTimeout(resolve, 100))
		}
		throw new Error(`Packaged server did not become ready: ${output}`)
	} finally {
		child.kill('SIGTERM')
		await closed
	}
}

/** Exercise an npm-installed archive, including the actual global command shim. */
export async function smokePackage(archive, expectedVersion, resources) {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-package-smoke-'),
	)
	const runInstalled = (command, args, options = {}) =>
		run(command, args, { cwd: temporary, ...options })
	try {
		const prefix = path.join(temporary, 'install')
		const resourceArchives =
			resources ??
			(await json(path.join(root, '.resource-packages/dependencies.json'))).map(
				(entry) => entry.archive,
			)
		await npm([
			'install',
			'--global',
			'--prefix',
			prefix,
			'--ignore-scripts',
			'--no-audit',
			'--no-fund',
			'--offline',
			...resourceArchives.filter(
				(file) => !path.basename(file).startsWith('bluink-web-'),
			),
			path.resolve(archive),
		])
		const packageRoot = path.join(
			prefix,
			...(process.platform === 'win32' ? [] : ['lib']),
			'node_modules',
			'@bluink',
			'cli',
		)
		const manifest = await json(path.join(packageRoot, 'package.json'))
		const cliVersion = expectedVersion ?? manifest.version
		const release = await json(path.join(packageRoot, 'release.json'))
		const frontend = await json(path.join(packageRoot, 'web/release.json'))
		if (
			frontend.version !== cliVersion ||
			release.frontend?.version !== cliVersion ||
			manifest.dependencies['@bluink/web']
		)
			throw new Error(
				'CLI frontend must be bundled at the matching version, without an npm Web dependency.',
			)
		if (
			manifest.name !== '@bluink/cli' ||
			manifest.version !== cliVersion ||
			(await json(path.join(packageRoot, 'dist/package.json'))).version !==
				cliVersion
		)
			throw new Error('Installed package identity or CLI version is incorrect.')
		for (const relative of [
			'bluink',
			'dist/index.js',
			'dist/src/native/lifecycle.js',
			'THIRD_PARTY_NOTICES.txt',
			'LICENSE',
			'README.md',
			'release.json',
			'web/LICENSE',
			'web/assets/index.html',
			'runtime/index.cjs',
			'vendor/themes/classic/manifest.json',
			'vendor/themes/editorial/manifest.json',
			'vendor/themes/classic/pdf/document.typ',
			...release.native.map((entry) => `runtime/${entry.file}`),
		]) {
			if (
				!(
					await stat(path.join(packageRoot, relative)).catch(() => null)
				)?.isFile()
			)
				throw new Error(`Missing packaged file: ${relative}`)
		}
		const installedRequire = createRequire(
			path.join(packageRoot, 'package.json'),
		)
		for (const dependency of release.dependencies) {
			const installed = await json(
				path.join(
					path.dirname(installedRequire.resolve(dependency.name)),
					'package.json',
				),
			)
			if (
				installed.version !== dependency.version ||
				manifest.dependencies[dependency.name] !== dependency.version
			)
				throw new Error(
					`Installed dependency differs from the verified release: ${dependency.name}`,
				)
		}
		if (
			await stat(
				path.join(packageRoot, 'dist/src/storage/file-project.js'),
			).catch(() => null)
		)
			throw new Error('The package contains the obsolete Node storage adapter.')
		const bin = process.platform === 'win32' ? prefix : path.join(prefix, 'bin')
		const env = {
			...process.env,
			PATH: `${bin}${path.delimiter}${process.env.PATH ?? ''}`,
		}
		const version = await runInstalled(
			process.platform === 'win32' ? 'cmd.exe' : 'bluink',
			process.platform === 'win32'
				? ['/d', '/c', 'bluink --version']
				: ['--version'],
			{ env, capture: true },
		)
		if (!version.includes(cliVersion))
			throw new Error(
				`Installed bluink command reports the wrong version: ${version}`,
			)
		const target = targets.find(
			(value) =>
				value.platform === process.platform && value.arch === process.arch,
		)
		if (!target) throw new Error('Cannot smoke test this host platform.')
		const { publicationRuntime } = await import(
			pathToFileURL(path.join(packageRoot, 'dist/src/native/addon.js')).href
		)
		const { typstBinary: typst, fontFolder } = publicationRuntime()
		if (!fontFolder.includes(`${path.sep}fonts${path.sep}assets${path.sep}pdf`))
			throw new Error(
				'PDF fonts did not resolve from the installed resource package.',
			)
		if (
			process.platform !== 'win32' &&
			((await stat(typst)).mode & 0o111) === 0
		)
			throw new Error('Packaged Typst is not executable.')
		await runInstalled(typst, ['--version'])
		const freshRoot = path.join(temporary, 'fresh-project')
		// Windows retains loaded DLLs until process exit. Own this addon in a
		// child so the parent can remove the isolated installation afterwards.
		await runInstalled(
			process.execPath,
			[
				'--input-type=module',
				'-e',
				`
import { nativeAddon, shutdownNativeDiagnostics } from ${JSON.stringify(pathToFileURL(path.join(packageRoot, 'dist/src/native/addon.js')).href)};
const fresh = new (nativeAddon().BluinkApplication)({projectRoot: ${JSON.stringify(freshRoot)}, timezone: 'UTC'});
try {
  const opened = JSON.parse(await fresh.initialize(false));
  if (opened.config.format !== 'bluink-project') throw new Error('The packaged native library did not initialize a fresh project.');
} finally {
  await fresh.stop();
  await shutdownNativeDiagnostics();
}
`,
			],
			{ cwd: temporary },
		)
		const project = path.join(temporary, 'project')
		await cp(path.join(root, 'backend/test-fixtures/pdf-project'), project, {
			recursive: true,
		})
		const cli = path.join(packageRoot, 'bluink')
		const sourcePage = '01a0e2a0-0000-7000-8000-000000000004'
		const publication = '01a0e7f5-d342-7f23-bd3d-6e27064a16cd'
		const html = path.join(temporary, 'article.html')
		const pdf = path.join(temporary, 'article.pdf')
		const book = path.join(temporary, 'book.pdf')
		const site = path.join(temporary, 'site')
		await runInstalled(process.execPath, [
			cli,
			'export',
			'html',
			'--project',
			project,
			'--note',
			sourcePage,
			'--out',
			html,
		])
		await runInstalled(process.execPath, [
			cli,
			'export',
			'pdf',
			'--project',
			project,
			'--note',
			sourcePage,
			'--out',
			pdf,
		])
		await runInstalled(process.execPath, [
			cli,
			'publication',
			'generate',
			'--project',
			project,
			'--id',
			publication,
			'--format',
			'pdf',
			'--output',
			book,
		])
		await runInstalled(process.execPath, [
			cli,
			'publication',
			'generate',
			'--project',
			project,
			'--id',
			publication,
			'--format',
			'site',
			'--output',
			site,
		])
		if (!(await readFile(html, 'utf8')).includes('A cited source paragraph.'))
			throw new Error('Packaged HTML output is missing source text.')
		for (const file of [pdf, book]) {
			if (!(await readFile(file)).subarray(0, 5).equals(Buffer.from('%PDF-')))
				throw new Error('Packaged PDF output has no PDF signature.')
		}
		if (
			!(await readFile(path.join(site, 'index.html'), 'utf8')).includes(
				'Introduction',
			)
		)
			throw new Error('Packaged site output is missing the root page.')
		await checkServer(cli, freshRoot)
		console.log(
			`Installed command, server and exports pass on ${process.platform}/${process.arch}.`,
		)
		return {
			platform: process.platform,
			arch: process.arch,
			status: 'passed',
			checks: [
				'global command',
				'bundled Typst',
				'fresh project',
				'note HTML/PDF',
				'Publication PDF/site',
				'HTTP UI hosting',
			],
		}
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	if (!process.argv[2]) throw new Error('Pass the packaged Bluink .tgz file.')
	await smokePackage(process.argv[2])
}
