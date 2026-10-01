# Image attachments

Status: proposed implementation contract; scope approved, written specification awaiting review.

## Intent and scope

Send screenshots and other images from Prompting Time to Codex or Claude without leaving
the app. Add one shared attachment path, not separate upload implementations for each
provider. Images remain private local conversation data; nothing is uploaded to a public
file host or copied into the project checkout.

This feature does not generate images, enable arbitrary Markdown image URLs, add document
uploads, make observed native child conversations editable, or persist drafts across app
restart. Existing text behavior and keyboard conventions remain unchanged.

## Composer and transcript

Offer an Attach images button with a native file picker, image paste, and drag-and-drop.
All three use the same native validation and import boundary. Show removable thumbnails
with filename, loading state, and an actionable error when import fails. Preserve clipboard
text when a paste has no supported image. A drop containing unsupported files reports the
unsupported items rather than silently treating them as images.

Allow text plus images and image-only messages. Disable sending while an import is pending.
Capture the target conversation and draft revision before importing so late results cannot
appear in another chat or attach themselves to a later message. Uncertain-send retries reuse
the complete original text and ordered attachment selection; editing creates a new request.

Submitted messages display bounded thumbnails that open a larger in-app preview. Copy
message remains a text operation; it must not expose base64 or private filesystem paths.
Use accessible labels and keyboard-operable remove/preview controls. Fit the existing
compact composer at minimum window width and increased zoom without overlapping controls.

While a native turn is running, an image draft cannot use text-only steering. Keep that
draft available and explain that images can be sent when the current turn ends. Do not
silently drop attachments or unexpectedly interrupt the provider.

## Validation, ownership, and storage

Initially support PNG, JPEG, and WebP. Accept at most four images per message, three MiB per
image, and five MiB of original image bytes in total. Reject images above 32 million decoded
pixels or 8,000 pixels on either axis. Validate actual format and decoded dimensions rather
than trusting filename extensions or browser MIME labels; reject unsupported animation.
Apply bounded decoding and encoded provider-frame checks as separate safeguards. These
conservative common limits can be raised after provider-specific evidence; never silently
resize or re-encode an original solely to make a send succeed.

The Rust backend copies each accepted import to immutable app-owned storage under Application
Support and creates a bounded thumbnail. It returns an opaque attachment identity; the
frontend cannot choose the stored path or supply arbitrary paths to a provider command.
Store the original digest, format, dimensions, byte count, and a sanitized display name once.
Keep originals and thumbnails outside repositories, logs, diagnostic events, and temporary
public artifacts. Removing or changing the source file must not break an accepted attachment.

Canonical messages and durable provider-run intents reference the same attachment identities.
The submission identity includes the ordered images, and accepted message/run references are
committed atomically. A crash between importing a file and accepting a message may leave an
unreferenced import; it must never leave an accepted message whose original was not persisted.
Cleanup removes only app-owned imports with no draft, accepted-message, or pending-run owner.
Archive preserves submitted attachments. Existing text-only conversations need no import.

## Provider boundary and continuity

Resolve attachment identities inside the backend immediately before provider dispatch. Use
the native Codex image input and Claude image content-block contracts verified for the
installed CLI; do not substitute filename text or a public URL. A known unsupported format/model,
missing stored original, invalid digest, or oversized serialized input fails before dispatch
with an actionable message, while preserving the draft and existing conversation.

The frozen request carries the images through recovery and safe fallback. On provider switches,
historical images selected for missing-context handoff remain associated with their messages.
Use the same bounded image envelope; if needed historical images do not fit, stop before
dispatch and explain that a new conversation or return to the existing provider is required.
Do not silently omit visual context. Same-provider continuation relies on its native session
for already-delivered history, without resending every previous image on every turn.

Keep attachment rendering separate from Markdown. Only images resolved by the app's attachment
API can be displayed; existing restrictions on arbitrary Markdown images and unsafe URLs stay.
Provider-generated images and import of images from external native histories are separate work.

## Verification and completion

First verify installed native payload schemas; then use invented fixtures for importer limits,
malformed bytes, path isolation, image-only sends, order-sensitive retries, navigation races,
steering rejection, restart recovery, fallback, and Codex/Claude switching. Test that deleting
the original source file does not affect a submitted image and that cleanup preserves all
referenced originals. UI checks cover picker/paste/drop, remove, preview, errors, keyboard
access, and narrow/zoomed layouts. Native delivery checks use tiny invented images only,
not screenshots, project data, or existing user conversations.

Do not mark the feature implemented from a UI preview or accepted native payload alone: confirm
that each provider can answer a simple visual question. Record any unverified native WebView,
resume, or model-specific behavior separately. Reconcile the main product specification and
provider evidence on completion; remove completed temporary execution plans.
