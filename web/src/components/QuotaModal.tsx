import { useEffect, useRef, useState } from 'react'
import { api, openPortal, type Billing } from '../lib/api'
import { formatDate } from '../lib/format'
import { useI18n } from '../lib/i18n'
import { useModalDialog } from '../lib/modal'

/**
 * 주최자의 이번 결제 기간 번역 시간이 떨어졌을 때(주최자에게만). 영상·음성은 계속되고 자막만 멈춘다.
 * 결제는 새 탭에서 한다(이 탭을 떠나면 회의가 끊기므로). 결제가 들어오면 서버가 알려 자막이 다시 켜진다.
 */
export function QuotaModal({ onClose }: { onClose: () => void }) {
  const { t, locale } = useI18n()
  const ref = useRef<HTMLDialogElement>(null)
  const [billing, setBilling] = useState<Billing | null>(null)
  useModalDialog(ref)
  useEffect(() => {
    void api.get<Billing>('/api/billing').then((r) => r.ok && setBilling(r.data))
  }, [])
  const [portalBusy, setPortalBusy] = useState(false)
  const [portalFailed, setPortalFailed] = useState(false)
  const hours = Math.round((billing?.includedSec ?? 0) / 3600)
  // 갱신 결제가 밀려 이번 기간의 포함 시간이 아직 없다: 다 쓴 것이 아니라 결제가 끝나면 들어온다.
  const unpaid = billing?.pastDue === true
  // 구독이 없는 사람(일일권을 다 썼거나 회의 중에 끝남): 시간 추가(구독자 전용) 대신 일일권을 하나 더 사거나 구독한다.
  const day = billing?.plan === 'day' || billing?.plan === 'none'

  function updatePayment() {
    // 이 탭을 떠나면 회의가 끊기므로 새 탭에서 연다. 응답을 기다린 뒤 열면 팝업 차단에 걸리므로 빈 탭부터 연다.
    const tab = window.open('', '_blank')
    if (!tab) return setPortalFailed(true)
    tab.opener = null
    setPortalBusy(true)
    setPortalFailed(false)
    void openPortal(tab).then((ok) => {
      setPortalBusy(false)
      setPortalFailed(!ok)
    })
  }

  return (
    <dialog ref={ref} className="sheet quota" aria-labelledby="quota-title" onClose={onClose}>
      <span className="quota-ring" aria-hidden="true">
        {hours ? t('checkout.hours', { n: String(hours) }) : ''}
      </span>
      <h2 id="quota-title">
        {t(unpaid ? 'billing.unpaid' : billing?.plan === 'day' ? 'quota.dayTitle' : billing?.plan === 'none' ? 'quota.noneTitle' : 'quota.title')}
      </h2>
      <p className="lead">
        {unpaid
          ? t('billing.unpaidBody')
          : day
            ? t(billing?.plan === 'day' ? 'quota.dayBody' : 'quota.noneBody')
            : billing
              ? t('quota.body', { date: formatDate(billing.resetsAt, locale) })
              : t('quota.bodyShort')}
      </p>
      {portalFailed && (
        <p className="form-error" role="alert">
          {t('account.billingFailed')}
        </p>
      )}
      <div className="quota-options">
        {unpaid && (
          <button type="button" className="quota-option primary" disabled={portalBusy} onClick={updatePayment}>
            <span>{t('checkout.updatePayment')}</span>
          </button>
        )}
        {day ? (
          <>
            <a className="quota-option primary" href="/checkout?item=day" target="_blank" rel="noreferrer">
              <span>{t('quota.dayAgain')}</span>
              <b>{t('quota.dayPrice')}</b>
            </a>
            <a className="quota-option" href="/checkout?item=basic" target="_blank" rel="noreferrer">
              <span>{t('quota.toBasic')}</span>
              <b>{t('quota.basicPrice')}</b>
            </a>
          </>
        ) : (
          <>
            <a className={`quota-option${unpaid ? '' : ' primary'}`} href="/checkout?item=hours&q=1" target="_blank" rel="noreferrer">
              <span>{t('quota.addOne')}</span>
              <b>$3</b>
            </a>
            <a className="quota-option" href="/checkout?item=hours&q=5" target="_blank" rel="noreferrer">
              <span>{t('quota.addFive')}</span>
              <b>$15</b>
            </a>
          </>
        )}
        {/* 결제가 밀린 구독은 Paddle이 요금제를 바꾸지 않는다. */}
        {billing?.plan === 'basic' && !unpaid && (
          <a className="quota-option" href="/checkout?item=pro" target="_blank" rel="noreferrer">
            <span>{t('quota.toPro')}</span>
            <b>{t('quota.proPrice')}</b>
          </a>
        )}
      </div>
      <button className="btn btn-quiet" onClick={() => ref.current?.close()}>
        {t('quota.continue')}
      </button>
    </dialog>
  )
}
