// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { createHash } from 'node:crypto'
import { auditDirectory } from './package-audit.mjs'
import { writeCliArchive } from './cli-archive.mjs'
import { json, npm, sha256, tar } from './release-utils.mjs'

/** Validate the supplied bytes rather than trusting a dependency record. */
export async function verifyArchive(record, options) {
	const bytes = await readFile(record.archive)
	const integrity = `sha512-${createHash('sha512').update(bytes).digest('base64')}`
	if (integrity !== record.integrity || sha256(bytes) !== record.sha256)
		throw new Error(`Dependency archive integrity mismatch: ${record.name}`)
	const inspected = await inspectArchive(record.archive, options)
	if (
		inspected.manifest.name !== record.name ||
		inspected.manifest.version !== record.version
	)
		throw new Error(`Dependency archive identity mismatch: ${record.name}`)
	return { ...record, ...inspected }
}

export async function inspectArchive(archive, options) {
	const temporary = await mkdtemp(path.join(os.tmpdir(), 'bluink-archive-'))
	try {
		const listing = (
			await tar(['-tzf', path.resolve(archive)], { capture: true })
		)
			.trim()
			.split('\n')
		if (
			listing.some(
				(file) =>
					!file.startsWith('package/') ||
					file.includes('..') ||
					file.includes('\\'),
			)
		)
			throw new Error('Unsafe path in package archive.')
		const types = (
			await tar(['-tvzf', path.resolve(archive)], { capture: true })
		)
			.trim()
			.split('\n')
		if (types.some((line) => !/^[d-]/.test(line)))
			throw new Error(
				'Links and special files are not allowed in package archives.',
			)
		await tar(['-xzf', path.resolve(archive), '-C', temporary], {
			capture: true,
		})
		const directory = path.join(temporary, 'package')
		const { onInspect, ...auditOptions } = options
		const files = await auditDirectory(directory, auditOptions)
		const result = {
			files,
			manifest: await json(path.join(directory, 'package.json')),
			metadata: await json(path.join(directory, 'release.json')).catch(
				(error) => {
					if (error.code === 'ENOENT') return undefined
					throw error
				},
			),
		}
		if (onInspect) result.inspection = await onInspect(directory, result)
		return result
	} finally {
		await rm(temporary, { recursive: true, force: true })
	}
}

export async function packDirectory(directory, destination, options) {
	const files = await auditDirectory(directory, options)
	await mkdir(destination, { recursive: true })
	const manifest = await json(path.join(directory, 'package.json'))
	if (manifest.name === '@bluink/cli') {
		if (!files.includes('npm-shrinkwrap.json'))
			throw new Error('CLI archives require a production dependency lock.')
		const filename = `bluink-cli-${manifest.version}.tgz`
		const archive = path.join(destination, filename)
		await writeCliArchive(directory, files, archive)
		const inspected = await inspectArchive(archive, options)
		const bytes = await readFile(archive)
		return {
			name: manifest.name,
			version: manifest.version,
			filename,
			archive,
			integrity: `sha512-${createHash('sha512').update(bytes).digest('base64')}`,
			sha256: sha256(bytes),
			...inspected,
		}
	}
	const output = JSON.parse(
		await npm(
			[
				'pack',
				path.resolve(directory),
				'--ignore-scripts',
				'--json',
				'--pack-destination',
				path.resolve(destination),
			],
			{ capture: true },
		),
	)
	const records = Array.isArray(output) ? output : Object.values(output)
	if (records.length !== 1)
		throw new Error('npm pack must produce exactly one archive.')
	const record = records[0]
	const archive = path.join(destination, record.filename)
	const inspected = await inspectArchive(archive, options)
	const bytes = await readFile(archive)
	const integrity = `sha512-${createHash('sha512').update(bytes).digest('base64')}`
	if (
		record.name !== inspected.manifest.name ||
		record.version !== inspected.manifest.version ||
		record.integrity !== integrity
	)
		throw new Error('Packed archive identity or integrity is inconsistent.')
	return {
		name: record.name,
		version: record.version,
		filename: record.filename,
		archive,
		integrity,
		sha256: sha256(bytes),
		...inspected,
	}
}

/** Resource packages currently have no further production dependencies. */
export function productionLock(manifest, dependencies) {
	const packages = {
		'': {
			name: manifest.name,
			version: manifest.version,
			license: manifest.license,
			dependencies: manifest.dependencies,
		},
	}
	for (const [name, version] of Object.entries(manifest.dependencies)) {
		const dependency = dependencies.find((entry) => entry.name === name)
		if (!dependency || dependency.version !== version)
			throw new Error(`Missing exact dependency archive: ${name}@${version}`)
		if (
			Object.keys(dependency.manifest.dependencies ?? {}).length ||
			Object.keys(dependency.manifest.optionalDependencies ?? {}).length
		)
			throw new Error(
				`Extend production dependency locking before adding transitive dependencies to ${name}.`,
			)
		packages[`node_modules/${name}`] = {
			version,
			resolved: `https://registry.npmjs.org/${name}/-/${name.split('/').at(-1)}-${version}.tgz`,
			integrity: dependency.integrity,
			license: dependency.manifest.license,
		}
	}
	return {
		name: manifest.name,
		version: manifest.version,
		lockfileVersion: 3,
		requires: true,
		packages,
	}
}
