export type MediaError = {
  kind: 'denied' | 'notFound' | 'busy' | 'unknown'
  device: 'camera' | 'both'
}
export type LocalMedia = { stream: MediaStream; error: MediaError | null }

const audio = { echoCancellation: true, noiseSuppression: true, autoGainControl: true }
const video = { width: 1280, height: 720 }

/** 카메라+마이크를 요청하고, 실패하면 마이크만, 그래도 실패하면 빈 스트림으로 입장할 수 있게 한다. */
export async function acquireLocalMedia(): Promise<LocalMedia> {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio, video })
    return { stream, error: null }
  } catch (videoErr) {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio })
      return { stream, error: { kind: classify(videoErr), device: 'camera' } }
    } catch (audioErr) {
      return { stream: new MediaStream(), error: { kind: classify(audioErr), device: 'both' } }
    }
  }
}

/** 회의 중 끝난(장치가 빠졌거나 권한이 회수된) 마이크·카메라를 같은 설정으로 다시 잡는다. 못 잡으면 예외. */
export async function reacquireTrack(kind: 'audio' | 'video'): Promise<MediaStreamTrack> {
  const stream = await navigator.mediaDevices.getUserMedia(kind === 'audio' ? { audio } : { video })
  const track = stream.getTracks()[0]
  if (!track) throw new DOMException('no track', 'NotFoundError')
  return track
}

function classify(err: unknown): MediaError['kind'] {
  const name = err instanceof DOMException ? err.name : ''
  if (name === 'NotAllowedError') return 'denied'
  if (name === 'NotFoundError') return 'notFound'
  if (name === 'NotReadableError') return 'busy'
  return 'unknown'
}
