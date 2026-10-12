// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { packDirectory, verifyArchive } from './package-artifacts.mjs'
import { webFileAllowed } from './package-audit.mjs'
import { stageFrontend } from './frontend-asset.mjs'
import { json, root, sha256, writeJson } from './release-utils.mjs'
import { publicIdentity, versionPattern } from './release-identity.mjs'
import { registryRecord } from './registry-publication.mjs'

export async function resourceAudit(kind) {
	if (!['web', 'fonts', 'typst'].includes(kind))
		throw new Error('Unknown resource package.')
	const pins =
		kind === 'typst'
			? await json(path.join(root, 'typst/sources.json'))
			: undefined
	return {
		allowed:
			kind === 'web'
				? webFileAllowed
				: (file) => resourceFileAllowed(kind, file),
		frontend: kind === 'web',
		trustedUpstream: Object.fromEntries(
			Object.entries(pins?.targets ?? {}).map(([target, spec]) => [
				`assets/${target}/${spec.binary}`,
				spec.binarySha256,
			]),
		),
	}
}

/** Fetch fixed registry archives; never run package builders or install scripts. */
export async function downloadResources(
	entries,
	destination,
	fetchArchive = fetch,
) {
	await mkdir(destination, { recursive: true })
	const records = []
	for (const entry of entries) {
		if (!/^[a-f0-9]{64}$/.test(entry.sha256))
			throw new Error('Frozen resource SHA-256 is required.')
		records.push(await downloadResource(entry, destination, fetchArchive))
	}
	return records
}

/** Initial private preparation pins already-published resource bytes. */
export async function resolvePublishedResources(
	pins,
	destination,
	{ lookup = registryRecord, fetchArchive = fetch } = {},
) {
	await mkdir(destination, { recursive: true })
	const records = []
	for (const pin of pins) {
		if (
			!['@bluink/fonts', '@bluink/typst'].includes(pin.name) ||
			!versionPattern.test(pin.version)
		)
			throw new Error('Specify an exact published resource version.')
		const published = await lookup(pin.name, pin.version)
		if (
			published?.name !== pin.name ||
			published.version !== pin.version ||
			!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(published.dist?.integrity)
		)
			throw new Error(`Publish the exact ${pin.name}@${pin.version} first.`)
		records.push(
			await downloadResource(
				{ ...pin, integrity: published.dist.integrity },
				destination,
				fetchArchive,
			),
		)
	}
	return records
}

async function downloadResource(entry, destination, fetchArchive) {
	const kind = entry.name?.split('/')[1]
	if (
		entry.name !== `@bluink/${kind}` ||
		!['fonts', 'typst'].includes(kind) ||
		!versionPattern.test(entry.version)
	)
		throw new Error('Invalid resource download identity.')
	const filename = `bluink-${kind}-${entry.version}.tgz`
	const archive = path.join(destination, filename)
	const response = await fetchArchive(
		`https://registry.npmjs.org/@bluink/${kind}/-/${kind}-${entry.version}.tgz`,
		{ signal: AbortSignal.timeout(120_000) },
	)
	if (!response.ok)
		throw new Error(
			`Publish the exact ${entry.name}@${entry.version} first: HTTP ${response.status}`,
		)
	const bytes = Buffer.from(await response.arrayBuffer())
	await writeFile(archive, bytes)
	return verifyArchive(
		{ ...entry, sha256: entry.sha256 ?? sha256(bytes), filename, archive },
		await resourceAudit(kind),
	)
}

/** Public source builds use npm-installed assets, with no resource generation. */
export async function stageInstalledResources({
	installRoot = root,
	packageRoots,
} = {}) {
	const cliRequire = createRequire(
		path.join(installRoot, 'backend/package.json'),
	)
	for (const kind of ['fonts', 'typst']) {
		const source =
			packageRoots?.[kind] ??
			path.dirname(cliRequire.resolve(`@bluink/${kind}`))
		await stageResourceDirectory(kind, source, installRoot)
	}
}

export async function stageResourceArchives(records, installRoot = root) {
	for (const record of records) {
		const kind = record.name.split('/')[1]
		if (!['fonts', 'typst'].includes(kind))
			throw new Error('Only Fonts and Typst can be staged as build resources.')
		await verifyArchive(record, {
			...(await resourceAudit(kind)),
			onInspect: (directory) =>
				stageResourceDirectory(kind, directory, installRoot),
		})
	}
}

async function stageResourceDirectory(kind, source, installRoot) {
	const contract = kind === 'fonts' ? 'catalogue.json' : 'sources.json'
	if (
		!(await readFile(path.join(source, contract))).equals(
			await readFile(path.join(installRoot, kind, contract)),
		)
	)
		throw new Error(
			`Installed ${kind} contract differs from public build inputs.`,
		)
	if (kind === 'typst') {
		const pins = await json(path.join(source, 'sources.json'))
		for (const [target, spec] of Object.entries(pins.targets))
			if (
				sha256(
					await readFile(path.join(source, 'assets', target, spec.binary)),
				) !== spec.binarySha256
			)
				throw new Error(`Installed Typst binary checksum mismatch: ${target}`)
	}
	if (path.resolve(source) !== path.join(installRoot, kind)) {
		const destination = path.join(installRoot, kind, 'assets')
		await rm(destination, { recursive: true, force: true })
		await cp(path.join(source, 'assets'), destination, { recursive: true })
	}
}

/** Portable artifact metadata: never transmit runner-specific absolute paths. */
export function resourceSummary({
	name,
	version,
	filename,
	integrity,
	sha256,
}) {
	return { name, version, filename, integrity, sha256 }
}

/** Consume the exact shared archives prepared by the single resource job. */
export async function readPreparedResources(input, directory, installRoot) {
	const entries = await json(path.join(directory, 'dependencies.json'))
	if (
		!Array.isArray(entries) ||
		entries.length !== 3 ||
		new Set(entries.map((entry) => entry.name)).size !== 3
	)
		throw new Error('Incomplete shared resource archive set.')
	const records = []
	const options = new Map()
	for (const entry of entries) {
		const expected =
			entry.name === '@bluink/web'
				? input.frontend
				: input.dependencies.find(
						(dependency) => dependency.name === entry.name,
					)
		if (
			!expected ||
			entry.filename !==
				`${entry.name.replace('@', '').replace('/', '-')}-${entry.version}.tgz` ||
			Object.keys(entry).some(
				(key) =>
					!['name', 'version', 'filename', 'integrity', 'sha256'].includes(key),
			) ||
			['name', 'version', 'integrity', 'sha256'].some(
				(key) => entry[key] !== expected[key],
			)
		)
			throw new Error(
				'Shared resource differs from frozen input or has unsafe metadata.',
			)
		const kind = entry.name.split('/')[1]
		const audit = await resourceAudit(kind)
		const record = { ...entry, archive: path.join(directory, entry.filename) }
		records.push(await verifyArchive(record, audit))
		options.set(entry.name, audit)
	}
	// Verify every archive before staging native test/compile assets.
	if (installRoot)
		await stageFrontend(
			records.find((entry) => entry.name === '@bluink/web'),
			path.join(installRoot, 'backend/web'),
		)
	if (installRoot)
		for (const record of records.filter(
			(entry) => entry.name !== '@bluink/web',
		))
			await verifyArchive(record, {
				...options.get(record.name),
				onInspect: async (directory) => {
					const destination = path.join(
						installRoot,
						record.name.split('/')[1],
						'assets',
					)
					await rm(destination, { recursive: true, force: true })
					await cp(path.join(directory, 'assets'), destination, {
						recursive: true,
					})
				},
			})
	return records
}

export function resourceFileAllowed(kind, file) {
	if (
		/^(?:package\.json|index\.(?:js|d\.ts)|sources\.json|LICENSE|README\.md)$/.test(
			file,
		)
	)
		return true
	if (kind === 'fonts')
		return /^(?:catalogue\.(?:json|js|d\.ts)|assets\/(?:manifest\.json|web\/(?:[\w-]+\.css|licenses\.json|licenses\/[\w-]+\.txt|[\w-]+\/[\w-]+\.woff2)|pdf\/[\w-]+\/(?:LICENSE|[\w-]+\.ttf)))$/.test(
			file,
		)
	return /^(?:licenses\/typst-assets-NOTICE|assets\/(?:(?:linux-x64|macos-arm64)\/(?:typst|LICENSE|NOTICE)|windows-x64\/(?:typst\.exe|LICENSE|NOTICE)))$/.test(
		file,
	)
}

export async function packResources(
	destination = path.join(root, '.resource-packages'),
	records = false,
	repository,
) {
	const archives = []
	for (const kind of ['fonts', 'typst']) {
		const archive = await packResource(kind, destination, { repository })
		archives.push(records ? archive : archive.archive)
	}
	return archives
}

/** Package one already-built resource without invoking either resource builder. */
export async function packResource(
	kind,
	destination,
	{ version, repository, sourceRoot = root } = {},
) {
	if (!['fonts', 'typst'].includes(kind))
		throw new Error('Choose fonts or typst.')
	const source = path.join(sourceRoot, kind)
	const manifest = await json(path.join(source, 'package.json'))
	version ??= manifest.version
	if (!versionPattern.test(version))
		throw new Error('Specify a calendar resource version.')
	const publicRepository =
		repository ?? manifest.repository?.url?.replace(/\.git$/, '')
	publicIdentity(version, { publicRepository })
	if (manifest.name !== `@bluink/${kind}`)
		throw new Error('Resource source has the wrong package identity.')
	if (
		!Array.isArray(manifest.files) ||
		manifest.files.some(
			(file) =>
				typeof file !== 'string' ||
				(!['assets', 'licenses'].includes(file) &&
					(!resourceFileAllowed(kind, file) || file.includes('/'))),
		)
	)
		throw new Error('Resource source has unsafe package files.')
	await mkdir(destination, { recursive: true })
	const pins =
		kind === 'typst' ? await json(path.join(source, 'sources.json')) : undefined
	const trustedUpstream = Object.fromEntries(
		Object.entries(pins?.targets ?? {}).map(([target, spec]) => [
			`assets/${target}/${spec.binary}`,
			spec.binarySha256,
		]),
	)
	const temporary = await mkdtemp(path.join(os.tmpdir(), 'bluink-resource-'))
	try {
		for (const file of manifest.files)
			await cp(path.join(source, file), path.join(temporary, file), {
				recursive: true,
			})
		if (kind === 'typst')
			for (const target of ['linux-x64', 'macos-arm64', 'windows-x64'])
				await rm(path.join(temporary, 'assets', target, 'README.md'), {
					force: true,
				})
		const published = Object.fromEntries(
			[
				'name',
				'version',
				'description',
				'type',
				'license',
				'exports',
				'files',
				'engines',
			]
				.filter((key) => manifest[key] !== undefined)
				.map((key) => [key, manifest[key]]),
		)
		published.version = version
		published.homepage = 'https://bluink.app'
		if (publicRepository)
			published.repository = {
				type: 'git',
				url: `${publicRepository}.git`,
				directory: kind,
			}
		published.publishConfig = { access: 'public' }
		await writeJson(path.join(temporary, 'package.json'), published)
		const archive = await packDirectory(temporary, destination, {
			trustedUpstream,
			allowed: (file) => resourceFileAllowed(kind, file),
			required: [
				'LICENSE',
				'README.md',
				'index.js',
				'sources.json',
				...(kind === 'fonts'
					? ['assets/web/inter.css', 'assets/pdf/inter/LICENSE']
					: [
							'assets/linux-x64/typst',
							'assets/macos-arm64/typst',
							'assets/windows-x64/typst.exe',
							'licenses/typst-assets-NOTICE',
						]),
			],
		})
		return archive
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	if (process.argv.slice(2).join(' ') !== 'stage-installed')
		throw new Error('Choose stage-installed.')
	await stageInstalledResources()
}
