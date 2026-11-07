// SPDX-License-Identifier: MPL-2.0
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { verifyArchive } from './package-artifacts.mjs'
import { cliFileAllowed } from './package-audit.mjs'
import { verifyPlatformArchive } from './platform-package.mjs'
import { nativePackageVersion, versionPattern } from './release-identity.mjs'
import { registryRecord } from './registry-publication.mjs'
import { packageName, sha256, targets } from './release-utils.mjs'

export function validateNativeSelection(selection) {
	if (
		!versionPattern.test(selection?.releaseVersion) ||
		selection.version !== nativePackageVersion(selection.releaseVersion) ||
		!/^[a-f0-9]{40}$/.test(selection.publicCommit) ||
		!Array.isArray(selection.packages) ||
		selection.packages.length !== targets.length ||
		Object.keys(selection).some(
			(key) =>
				!['releaseVersion', 'version', 'publicCommit', 'packages'].includes(
					key,
				),
		)
	)
		throw new Error('Invalid published native release selection.')
	for (const target of targets) {
		const record = selection.packages.find(
			({ name }) => name === target.addonPackage,
		)
		if (
			record?.version !== selection.version ||
			!/^[a-f0-9]{64}$/.test(record.sha256) ||
			!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(record.integrity) ||
			Object.keys(record).some(
				(key) => !['name', 'version', 'integrity', 'sha256'].includes(key),
			)
		)
			throw new Error(
				`Missing exact native package identity: ${target.addonPackage}`,
			)
	}
	return selection
}

async function registryBytes(name, version) {
	const file = name.split('/').at(-1)
	const response = await fetch(
		`https://registry.npmjs.org/${name}/-/${file}-${version}.tgz`,
		{ signal: AbortSignal.timeout(120_000) },
	)
	if (!response.ok)
		throw new Error(
			`Cannot download ${name}@${version}: HTTP ${response.status}`,
		)
	return Buffer.from(await response.arrayBuffer())
}

export async function publishedNativeSelection(productVersion) {
	if (!versionPattern.test(productVersion))
		throw new Error('Specify an exact published bluink version.')
	const published = await registryRecord(packageName, productVersion)
	if (published?.version !== productVersion || published.name !== packageName)
		throw new Error(
			`Publish ${packageName}@${productVersion} before reusing it.`,
		)
	const bytes = await registryBytes(packageName, productVersion)
	const directory = await mkdtemp(
		path.join(os.tmpdir(), 'bluink-native-origin-'),
	)
	try {
		const archive = path.join(directory, 'bluink.tgz')
		await writeFile(archive, bytes)
		const { metadata } = await verifyArchive(
			{
				name: packageName,
				version: productVersion,
				archive,
				sha256: sha256(bytes),
				integrity: published.dist.integrity,
			},
			{ allowed: cliFileAllowed },
		)
		if (metadata?.distribution !== 'all-platforms')
			throw new Error('Select an official Bluink release.')
		return validateNativeSelection({
			releaseVersion: metadata.nativeOrigin?.releaseVersion ?? productVersion,
			version: metadata.native[0]?.version,
			publicCommit:
				metadata.nativeOrigin?.publicCommit ?? metadata.publicCommit,
			packages: metadata.native.map(({ name, version, integrity, sha256 }) => ({
				name,
				version,
				integrity,
				sha256,
			})),
		})
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
}

export async function downloadNativeComponents(selection, directory) {
	validateNativeSelection(selection)
	await mkdir(directory, { recursive: true })
	for (const { name, version } of selection.packages) {
		await writeFile(
			path.join(directory, nativeFilename(name, version)),
			await registryBytes(name, version),
		)
	}
}

function nativeFilename(name, version) {
	return `${name.replace('@', '').replace('/', '-')}-${version}.tgz`
}

export async function readNativeComponents(
	selection,
	directory,
	publicRepository,
) {
	validateNativeSelection(selection)
	const records = []
	for (const target of targets) {
		const record = selection.packages.find(
			({ name }) => name === target.addonPackage,
		)
		const filename = nativeFilename(record.name, record.version)
		const archive = path.join(directory, filename)
		const {
			manifest,
			addon: native,
			notices,
		} = await verifyPlatformArchive({ ...record, archive }, target, {
			version: selection.version,
			publicCommit: selection.publicCommit,
			publicRepository,
		})
		records.push({ ...record, filename, archive, manifest, native, notices })
	}
	return records
}
