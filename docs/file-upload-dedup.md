# File upload deduplication

Direct POST /files uploads store a SHA-256 digest in files.sha256. Each request creates its
own file row and path, preserving its name, content type, description, and independent delete
behavior. For matching bytes owned by the same user, the storage layer hard links the new path
to an existing uploaded file. The database transaction takes a per-user, per-digest advisory
lock so concurrent uploads select the same underlying bytes. If hard linking is unavailable,
the request succeeds by writing a separate copy.

Generated images, artifacts, and skill files have a null digest and do not join this policy.
Existing file rows also keep a null digest; there is no migration-time filesystem scan.
The digest is internal and is not returned in the file API.

DELETE /files/{id} still soft-deletes only that upload row. Artifact deletion retains its
file bytes because older project-source rows could point to an uploaded attachment; physical
garbage collection needs a separate reference-aware design.
