import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import {
	fileHash,
	json,
	nativeHostTarget,
	npm,
	packageLauncher,
	packageName,
	platformAddon,
	root,
	run,
	targets,
} from './release-utils.mjs'

/** HTTP media types may carry parameters such as a charset. */
export function hasMediaType(response, expected) {
	return (
		response.ok &&
		response.headers
			.get('content-type')
			?.split(';', 1)[0]
			.trim()
			.toLowerCase() === expected
	)
}

/** The installed UI, its bundles and its fonts, with no source or debug paths. */
async function checkInterface(origin) {
	const response = await fetch(origin, { signal: AbortSignal.timeout(10_000) })
	const html = await response.text()
	if (!response.ok || !html.includes('<html') || !html.includes('/assets/'))
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
		const resource = await fetch(new URL(asset, origin))
		const expectedType = asset.split('?')[0].endsWith('.css')
			? 'text/css'
			: 'text/javascript'
		if (!hasMediaType(resource, expectedType))
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
		const response = await fetch(new URL(forbidden, origin))
		if (response.status !== 404)
			throw new Error(
				`Production source/debug path is accessible: ${forbidden}`,
			)
	}
	// Fonts are loaded by CSS at runtime and do not appear in index.html.
	const css = await fetch(new URL('/fonts/inter.css', origin))
	if (!hasMediaType(css, 'text/css'))
		throw new Error(
			'Packaged font stylesheet is missing or has an incorrect MIME type.',
		)
	const fontPath = (await css.text()).match(/url\('\.\/([^']+\.woff2)'\)/)?.[1]
	if (!fontPath) throw new Error('Packaged font stylesheet has no font asset.')
	const font = await fetch(new URL(`/fonts/${fontPath}`, origin))
	if (
		!hasMediaType(font, 'font/woff2') ||
		!(await font.arrayBuffer()).byteLength
	)
		throw new Error(
			'Packaged UI font is missing or served with an incorrect MIME type.',
		)
}

/** Start a plain server on a fresh project, check it, then stop it through
 * standard input as a supervisor would. Windows cannot signal the child. */
async function checkServer(launcher, project, env) {
	const child = spawn(
		process.execPath,
		[launcher, 'server', '--no-tui', '--port', '0', '--project', project],
		{ cwd: path.dirname(project), env, stdio: ['pipe', 'pipe', 'pipe'] },
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
	const closed = new Promise((resolve) =>
		child.once('close', (code, signal) => resolve({ code, signal })),
	)
	try {
		let origin
		for (let attempt = 0; !origin; attempt++) {
			if (failure || child.exitCode !== null || attempt === 1200)
				throw new Error(
					`Packaged server did not start: ${failure?.message ?? output}`,
				)
			origin = /Server started at (http:\/\/127\.0\.0\.1:\d+)\//.exec(
				output,
			)?.[1]
			if (!origin) await new Promise((resolve) => setTimeout(resolve, 100))
		}
		await checkInterface(`${origin}/`)
	} catch (error) {
		child.kill()
		await closed
		throw error
	}
	child.stdin.end('stop\n')
	const timer = setTimeout(() => child.kill(), 30_000)
	const { code, signal } = await closed
	clearTimeout(timer)
	if (code !== 0)
		throw new Error(
			`Packaged server did not stop cleanly on stdin (${signal ?? `exit ${code}`}): ${output}`,
		)
	if (!(await stat(project).catch(() => null))?.isDirectory())
		throw new Error('The packaged server did not create a fresh project.')
}

/** Exercise an npm-installed archive through the actual global command shim.
 * `resources` are the dependency archives and this machine's platform package. */
export async function smokePackage(archive, expectedVersion, resources) {
	const temporary = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-package-smoke-'),
	)
	const env = {
		...process.env,
		HOME: path.join(temporary, 'user-home'),
		USERPROFILE: path.join(temporary, 'user-home'),
	}
	const runInstalled = (command, args, options = {}) =>
		run(command, args, { cwd: temporary, env, ...options })
	try {
		const prefix = path.join(temporary, 'install')
		const resourceArchives =
			resources ??
			(await json(path.join(root, '.resource-packages/dependencies.json'))).map(
				(entry) => entry.archive,
			)
		// No package has an install script, so npm's default script policy applies
		// unchanged. The shrinkwrap pins every dependency's integrity, and the
		// archives installed alongside satisfy those pins offline.
		await npm([
			'install',
			'--global',
			'--prefix',
			prefix,
			'--no-audit',
			'--no-fund',
			'--offline',
			// Absolute paths: npm reads a relative `dir/file.tgz` as a GitHub repository.
			...resourceArchives.map((file) => path.resolve(file)),
			path.resolve(archive),
		])
		const packageRoot = path.join(
			prefix,
			...(process.platform === 'win32' ? [] : ['lib']),
			'node_modules',
			packageName,
		)
		const manifest = await json(path.join(packageRoot, 'package.json'))
		const cliVersion = expectedVersion ?? manifest.version
		const release = await json(path.join(packageRoot, 'release.json'))
		if (
			manifest.name !== packageName ||
			manifest.version !== cliVersion ||
			manifest.bin?.bluink !== packageLauncher ||
			manifest.scripts !== undefined
		)
			throw new Error('Installed package identity or CLI version is incorrect.')
		for (const relative of [
			packageLauncher,
			'THIRD_PARTY_NOTICES.txt',
			'LICENSE',
			'README.md',
			'release.json',
			'vendor/workspace-starter.json',
			'vendor/themes/classic/manifest.json',
			'vendor/themes/editorial/manifest.json',
			'vendor/themes/classic/html/article.html',
		]) {
			if (
				!(
					await stat(path.join(packageRoot, relative)).catch(() => null)
				)?.isFile()
			)
				throw new Error(`Missing packaged file: ${relative}`)
		}
		const target = nativeHostTarget()
		const host = release.native.find(
			(entry) => entry.addon.target === target.rust,
		)
		if (!host || host.name !== target.addonPackage)
			throw new Error(`The package has no ${target.rust} platform package.`)
		const installedRequire = createRequire(
			path.join(packageRoot, 'package.json'),
		)
		const installedRoot = async (name) =>
			path.dirname(
				installedRequire.resolve(
					name === '@bluink/web' ? `${name}/package.json` : name,
				),
			)
		const webRoot = await installedRoot('@bluink/web')
		const frontend = await json(path.join(webRoot, 'release.json'))
		if (
			frontend.version !== release.frontend?.version ||
			frontend.version !== manifest.dependencies['@bluink/web'] ||
			frontend.contractHash !== release.contractHash ||
			!(await stat(path.join(webRoot, 'assets/index.html'))).isFile()
		)
			throw new Error('Installed Web package differs from the pinned frontend.')
		// npm installs only this machine's platform package, at the pinned version.
		const platformRoot = path.dirname(
			installedRequire.resolve(`${target.addonPackage}/package.json`),
		)
		const platform = await json(path.join(platformRoot, 'package.json'))
		if (
			platform.version !== host.version ||
			manifest.optionalDependencies?.[target.addonPackage] !== host.version ||
			(await fileHash(path.join(platformRoot, platformAddon))) !==
				host.addon.sha256
		)
			throw new Error(
				`Installation did not place the verified ${target.rust} addon.`,
			)
		for (const other of targets.filter((entry) => entry !== target))
			if (
				(() => {
					try {
						return installedRequire.resolve(
							`${other.addonPackage}/package.json`,
						)
					} catch {
						return undefined
					}
				})()
			)
				throw new Error(
					`npm installed another platform's package: ${other.addonPackage}`,
				)
		const launcher = path.join(packageRoot, packageLauncher)
		// The global shim: npm's cmd shim on Windows, a symlink elsewhere.
		const bin = process.platform === 'win32' ? prefix : path.join(prefix, 'bin')
		const shimEnv = {
			...env,
			PATH: `${bin}${path.delimiter}${process.env.PATH ?? ''}`,
		}
		const shim = (args) =>
			runInstalled(
				process.platform === 'win32' ? 'cmd.exe' : 'bluink',
				process.platform === 'win32'
					? ['/d', '/c', `bluink ${args.join(' ')}`]
					: args,
				{ env: shimEnv, capture: true },
			)
		const version = await shim(['--version'])
		if (version.trim() !== `${manifest.name} v${cliVersion}`)
			throw new Error(
				`Installed bluink command reports the wrong version: ${version}`,
			)
		const buildInfo = JSON.parse(await shim(['build-info']))
		const expected = {
			...host.addon,
			version: host.version,
		}
		if (
			['sourceDigest', 'target', 'version', 'mode', 'publicCommit'].some(
				(key) => buildInfo[key] !== expected[key],
			)
		)
			throw new Error(
				`Installed bluink reports a different build: ${JSON.stringify(buildInfo)}`,
			)
		for (const dependency of release.dependencies) {
			const installed = await json(
				path.join(await installedRoot(dependency.name), 'package.json'),
			)
			if (
				installed.version !== dependency.version ||
				manifest.dependencies[dependency.name] !== dependency.version
			)
				throw new Error(
					`Installed dependency differs from the verified release: ${dependency.name}`,
				)
		}
		const wordFonts = await json(
			path.join(
				await installedRoot('@bluink/fonts'),
				'assets/docx/manifest.json',
			),
		)
		if (wordFonts.schemaVersion !== 1 || !wordFonts.fonts.length)
			throw new Error('Installed Word font manifest is missing or unsupported.')
		const project = path.join(temporary, 'project')
		const cli = (...args) => runInstalled(process.execPath, [launcher, ...args])
		await cli(
			'import',
			'--source',
			path.join(root, 'backend/test-fixtures/pdf-project'),
			'--destination',
			project,
		)
		const sourcePage = '01a0e2a0-0000-7000-8000-000000000004'
		const publication = '01a0e7f5-d342-7f23-bd3d-6e27064a16cd'
		const html = path.join(temporary, 'article.html')
		const pdf = path.join(temporary, 'article.pdf')
		const book = path.join(temporary, 'book.pdf')
		const word = path.join(temporary, 'article.docx')
		const wordBook = path.join(temporary, 'book.docx')
		const wordLinked = path.join(temporary, 'linked.docx')
		const site = path.join(temporary, 'site')
		const exportNote = (format, note, out, ...options) =>
			cli(
				'export',
				format,
				'--project',
				project,
				'--note',
				note,
				...options,
				'--out',
				out,
			)
		const generate = (format, output) =>
			cli(
				'publication',
				'generate',
				'--project',
				project,
				'--id',
				publication,
				'--format',
				format,
				'--output',
				output,
			)
		await exportNote('docx', sourcePage, word)
		await generate('docx', wordBook)
		await exportNote(
			'docx',
			'01a0e2a0-0000-7000-8000-000000000001',
			wordLinked,
			'--include-linked',
		)
		for (const file of [word, wordBook, wordLinked]) {
			const bytes = await readFile(file)
			if (
				!bytes.subarray(0, 4).equals(Buffer.from('504b0304', 'hex')) ||
				!bytes.includes(Buffer.from('word/fontTable.xml'))
			)
				throw new Error(
					'Installed Word export is missing its native document or embedded fonts.',
				)
		}
		await exportNote('html', sourcePage, html)
		await exportNote('pdf', sourcePage, pdf)
		await generate('pdf', book)
		await generate('site', site)
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
		await checkServer(launcher, path.join(temporary, 'fresh-project'), env)
		console.log(
			`Installed command, server and exports pass on ${process.platform}/${process.arch}.`,
		)
		return {
			platform: process.platform,
			arch: process.arch,
			status: 'passed',
			checks: [
				'npm installs only the matching platform addon package',
				'no install scripts',
				'global command version and build identity',
				'note HTML/PDF',
				'note/linked/Publication DOCX with embedded fonts',
				'Publication PDF/site',
				'fresh project HTTP UI hosting',
				'graceful stop on standard input',
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
	const [archive, ...platforms] = process.argv.slice(2)
	if (!archive || !platforms.length)
		throw new Error(
			"Pass the packaged bluink .tgz file and this platform's @bluink/cli-* .tgz file.",
		)
	const resources = (
		await json(path.join(root, '.resource-packages/dependencies.json'))
	).map((entry) => entry.archive)
	await smokePackage(archive, undefined, [...platforms, ...resources])
}
