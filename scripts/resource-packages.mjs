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

const fontsPackage = '@bluink/fonts'
const webPackage = '@bluink/web'

function resourceName(name) {
	if (name !== fontsPackage && name !== webPackage)
		throw new Error('Unknown resource package.')
	return name.split('/')[1]
}

export function resourceAudit(kind) {
	if (!['web', 'fonts'].includes(kind))
		throw new Error('Unknown resource package.')
	return {
		allowed: kind === 'web' ? webFileAllowed : resourceFileAllowed,
		frontend: kind === 'web',
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
		resourceName(pin.name)
		if (!versionPattern.test(pin.version))
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
	const kind = resourceName(entry.name)
	if (!versionPattern.test(entry.version))
		throw new Error('Invalid resource download identity.')
	const filename = `bluink-${kind}-${entry.version}.tgz`
	const archive = path.join(destination, filename)
	const response = await fetchArchive(
		`https://registry.npmjs.org/${entry.name}/-/${kind}-${entry.version}.tgz`,
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
		resourceAudit(kind),
	)
}

/** Public source builds use npm-installed assets, with no resource generation. */
export async function stageInstalledResources({
	installRoot = root,
	packageRoot,
} = {}) {
	const cliRequire = createRequire(
		path.join(installRoot, 'backend/package.json'),
	)
	await stageResourceDirectory(
		packageRoot ?? path.dirname(cliRequire.resolve(fontsPackage)),
		installRoot,
	)
}

export async function stageResourceArchives(records, installRoot = root) {
	for (const record of records) {
		if (record.name !== fontsPackage)
			throw new Error('Only Fonts can be staged as a build resource.')
		await verifyArchive(record, {
			...resourceAudit('fonts'),
			onInspect: (directory) => stageResourceDirectory(directory, installRoot),
		})
	}
}

async function stageResourceDirectory(source, installRoot) {
	if (
		!(await readFile(path.join(source, 'catalogue.json'))).equals(
			await readFile(path.join(installRoot, 'fonts/catalogue.json')),
		)
	)
		throw new Error(
			'Installed fonts contract differs from public build inputs.',
		)
	if (path.resolve(source) !== path.join(installRoot, 'fonts')) {
		const destination = path.join(installRoot, 'fonts/assets')
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
		entries.length !== 2 ||
		new Set(entries.map((entry) => entry.name)).size !== 2
	)
		throw new Error('Incomplete shared resource archive set.')
	const records = []
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
		const record = { ...entry, archive: path.join(directory, entry.filename) }
		records.push(
			await verifyArchive(record, resourceAudit(entry.name.split('/')[1])),
		)
	}
	// Verify every archive before staging native test/compile assets.
	if (installRoot) {
		await stageFrontend(
			records.find((entry) => entry.name === '@bluink/web'),
			path.join(installRoot, 'backend/web'),
		)
		await verifyArchive(
			records.find((entry) => entry.name === fontsPackage),
			{
				...resourceAudit('fonts'),
				onInspect: async (directory) => {
					const destination = path.join(installRoot, 'fonts/assets')
					await rm(destination, { recursive: true, force: true })
					await cp(path.join(directory, 'assets'), destination, {
						recursive: true,
					})
				},
			},
		)
	}
	return records
}

/** Files allowed in the published @bluink/fonts package. */
export function resourceFileAllowed(file) {
	return /^(?:package\.json|index\.(?:js|d\.ts)|sources\.json|LICENSE|README\.md|catalogue\.(?:json|js|d\.ts)|assets\/(?:manifest\.json|web\/(?:[\w-]+\.css|licenses\.json|licenses\/[\w-]+\.txt|[\w-]+\/[\w-]+\.woff2)|pdf\/[\w-]+\/(?:LICENSE|[\w-]+\.ttf)|docx\/(?:manifest\.json|[\w-]+\/(?:LICENSE|[\w-]+\.ttf))))$/.test(
		file,
	)
}

/** Package the already-built Fonts resource without invoking its builder. */
export async function packResource(
	destination,
	{ version, repository, sourceRoot = root } = {},
) {
	const source = path.join(sourceRoot, 'fonts')
	const manifest = await json(path.join(source, 'package.json'))
	version ??= manifest.version
	if (!versionPattern.test(version))
		throw new Error('Specify a calendar resource version.')
	const publicRepository =
		repository ?? manifest.repository?.url?.replace(/\.git$/, '')
	publicIdentity(version, { publicRepository })
	if (manifest.name !== fontsPackage)
		throw new Error('Resource source has the wrong package identity.')
	if (
		!Array.isArray(manifest.files) ||
		manifest.files.some(
			(file) =>
				typeof file !== 'string' ||
				(file !== 'assets' &&
					(!resourceFileAllowed(file) || file.includes('/'))),
		)
	)
		throw new Error('Resource source has unsafe package files.')
	await mkdir(destination, { recursive: true })
	const temporary = await mkdtemp(path.join(os.tmpdir(), 'bluink-resource-'))
	try {
		for (const file of manifest.files)
			await cp(path.join(source, file), path.join(temporary, file), {
				recursive: true,
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
				directory: 'fonts',
			}
		published.publishConfig = { access: 'public' }
		await writeJson(path.join(temporary, 'package.json'), published)
		return await packDirectory(temporary, destination, {
			allowed: resourceFileAllowed,
			required: [
				'LICENSE',
				'README.md',
				'index.js',
				'sources.json',
				'assets/web/inter.css',
				'assets/pdf/inter/LICENSE',
				'assets/docx/manifest.json',
				'assets/docx/inter/inter-400-normal.ttf',
			],
		})
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
