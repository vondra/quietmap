// A photo as the map keeps it: drawn from the browser's own decoding to a JPEG, upright and at most
// 4096 pixels a side (a 12-megapixel phone photo stays whole; iOS draws no canvas over 16.7
// megapixels), so that none of the camera's metadata, its position among it, leaves the phone. The
// server drops it again (server/src/media/check.ts).
const PHOTO_SIDE_MAX = 4096
const JPEG_QUALITY = 0.9

/** `file` drawn again as a JPEG, or null when this browser cannot read it (a HEIC outside Safari). */
export async function jpegOf(file: Blob): Promise<{ blob: Blob, width: number, height: number } | null> {
  const url = URL.createObjectURL(file)
  try {
    const image = new Image()
    image.src = url
    await image.decode()
    const scale = Math.min(1, PHOTO_SIDE_MAX / Math.max(image.naturalWidth, image.naturalHeight))
    const canvas = document.createElement('canvas')
    canvas.width = Math.round(image.naturalWidth * scale)
    canvas.height = Math.round(image.naturalHeight * scale)
    const context = canvas.getContext('2d')
    if (!context) return null
    context.imageSmoothingQuality = 'high'
    context.drawImage(image, 0, 0, canvas.width, canvas.height)
    const blob = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, 'image/jpeg', JPEG_QUALITY))
    return blob && { blob, width: canvas.width, height: canvas.height }
  } catch {
    return null
  } finally {
    URL.revokeObjectURL(url)
  }
}
