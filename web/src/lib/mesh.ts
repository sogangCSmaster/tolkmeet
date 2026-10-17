/**
 * 참가자마다 RTCPeerConnection 하나씩 두는 P2P mesh.
 * 양쪽이 동시에 offer를 보내는 경우는 "perfect negotiation" 패턴으로 처리한다.
 * https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API/Perfect_negotiation
 */

export type SignalData = {
  description?: RTCSessionDescriptionInit
  candidate?: RTCIceCandidateInit
}

type PeerLink = {
  pc: RTCPeerConnection
  polite: boolean
  makingOffer: boolean
  ignoreOffer: boolean
  audioSender: RTCRtpSender | null
  videoSender: RTCRtpSender | null
  remote: MediaStream
  // 같은 상대의 시그널은 도착 순서대로 하나씩 처리해야 한다(sRD 완료 전 candidate 추가 방지).
  queue: Promise<void>
}

export class Mesh {
  private links = new Map<string, PeerLink>()
  private videoTrack: MediaStreamTrack | null
  private selfId: string
  private local: MediaStream
  private iceServers: RTCIceServer[]
  private sendSignal: (to: string, data: SignalData) => void
  private onRemoteStream: (peerId: string, stream: MediaStream) => void

  constructor(
    selfId: string,
    local: MediaStream,
    iceServers: RTCIceServer[],
    sendSignal: (to: string, data: SignalData) => void,
    onRemoteStream: (peerId: string, stream: MediaStream) => void,
  ) {
    this.selfId = selfId
    this.local = local
    this.iceServers = iceServers
    this.sendSignal = sendSignal
    this.onRemoteStream = onRemoteStream
    this.videoTrack = local.getVideoTracks()[0] ?? null
  }

  /** 새로 입장한 쪽이 기존 참가자들에게 먼저 연결한다. 기존 참가자는 offer를 받을 때 링크를 만든다. */
  connect(peerId: string) {
    this.ensure(peerId)
  }

  handleSignal(from: string, data: SignalData) {
    const link = this.ensure(from)
    link.queue = link.queue
      .then(() => this.applySignal(link, from, data))
      .catch((err) => console.error('signal handling failed', from, err))
  }

  /** 카메라 ↔ 화면공유 전환(또는 다시 잡은 카메라). 미리 만든 비디오 송신자의 트랙만 교체한다. */
  async setVideoTrack(track: MediaStreamTrack | null) {
    this.videoTrack = track
    await this.replaceAll((link) => link.videoSender, track)
  }

  /** 다시 잡은 마이크로 바꾼다. 이후 새로 맺는 연결은 `local`의 지금 오디오 트랙을 쓴다. */
  async setAudioTrack(track: MediaStreamTrack | null) {
    await this.replaceAll((link) => link.audioSender, track)
  }

  private async replaceAll(sender: (link: PeerLink) => RTCRtpSender | null, track: MediaStreamTrack | null) {
    const results = await Promise.allSettled(
      [...this.links.values()].map(async (link) => {
        try {
          await sender(link)?.replaceTrack(track)
        } catch (err) {
          // 전환 도중 퇴장한 참가자는 더 이상 트랙을 받을 대상이 아니다.
          if (link.pc.signalingState !== 'closed') throw err
        }
      }),
    )
    // 다른 참가자의 늦은 교체가 실패 후 복원 작업을 덮어쓰지 않도록 모두 기다린다.
    const failed = results.find((result) => result.status === 'rejected')
    if (failed?.status === 'rejected') throw failed.reason
  }

  /** 상대 음성을 받는 오디오 수신기. 에코 판단(`FarEnd`)이 오디오 레벨을 읽는다. */
  audioReceivers(): RTCRtpReceiver[] {
    // 연결마다 오디오 구간이 둘이다(내가 보내는 것, 상대가 보내는 것). 받는 쪽만 고른다.
    return [...this.links.values()].flatMap((link) =>
      link.pc
        .getTransceivers()
        .filter((t) => t.receiver.track.kind === 'audio' && (t.currentDirection === 'recvonly' || t.currentDirection === 'sendrecv'))
        .map((t) => t.receiver),
    )
  }

  /** 상대별 받는 오디오 수신기. 누가 말하고 있는지(말하는 테두리) 표시에 쓴다. */
  audioReceiversByPeer(): [string, RTCRtpReceiver[]][] {
    return [...this.links.entries()].map(([id, link]) => [
      id,
      link.pc
        .getTransceivers()
        .filter((t) => t.receiver.track.kind === 'audio' && (t.currentDirection === 'recvonly' || t.currentDirection === 'sendrecv'))
        .map((t) => t.receiver),
    ])
  }

  remove(peerId: string) {
    this.links.get(peerId)?.pc.close()
    this.links.delete(peerId)
  }

  close() {
    for (const link of this.links.values()) link.pc.close()
    this.links.clear()
  }

  private ensure(peerId: string): PeerLink {
    const existing = this.links.get(peerId)
    if (existing) return existing

    const pc = new RTCPeerConnection({ iceServers: this.iceServers })
    const link: PeerLink = {
      pc,
      // 어느 쪽이 양보할지 양쪽이 같은 결론을 내기만 하면 된다.
      polite: this.selfId < peerId,
      makingOffer: false,
      ignoreOffer: false,
      audioSender: null,
      videoSender: null,
      remote: new MediaStream(),
      queue: Promise.resolve(),
    }
    this.links.set(peerId, link)

    // 장치나 권한이 없어도 수신용 미디어 구간을 만들어 먼저 협상할 수 있어야 한다.
    link.audioSender = pc.addTransceiver(this.local.getAudioTracks()[0] ?? 'audio', {
      direction: 'sendrecv',
      streams: [this.local],
    }).sender
    link.videoSender = pc.addTransceiver(this.videoTrack ?? 'video', {
      direction: 'sendrecv',
      streams: [this.local],
    }).sender

    pc.onnegotiationneeded = async () => {
      try {
        link.makingOffer = true
        await pc.setLocalDescription()
        this.sendSignal(peerId, { description: pc.localDescription!.toJSON() })
      } catch (err) {
        console.error('negotiation failed', peerId, err)
      } finally {
        link.makingOffer = false
      }
    }
    pc.onicecandidate = ({ candidate }) => {
      if (candidate) this.sendSignal(peerId, { candidate: candidate.toJSON() })
    }
    pc.ontrack = ({ track }) => {
      link.remote.addTrack(track)
      // React가 변경을 감지하도록 새 MediaStream 객체로 알린다.
      this.onRemoteStream(peerId, new MediaStream(link.remote.getTracks()))
    }
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'failed') pc.restartIce()
    }
    return link
  }

  private async applySignal(link: PeerLink, from: string, data: SignalData) {
    const { pc } = link
    if (data.description) {
      const collision = data.description.type === 'offer' && (link.makingOffer || pc.signalingState !== 'stable')
      link.ignoreOffer = !link.polite && collision
      if (link.ignoreOffer) return

      await pc.setRemoteDescription(data.description)
      if (data.description.type === 'offer') {
        await pc.setLocalDescription()
        this.sendSignal(from, { description: pc.localDescription!.toJSON() })
      }
    } else if (data.candidate) {
      try {
        await pc.addIceCandidate(data.candidate)
      } catch (err) {
        // 무시한 offer에 딸린 candidate는 실패해도 정상이다.
        if (!link.ignoreOffer) throw err
      }
    }
  }
}
