import { forwardRef } from 'react'

interface FloatingCardProps {
  children: React.ReactNode
  className?: string
}

const FloatingCard = forwardRef<HTMLDivElement, FloatingCardProps>(({ children, className = '' }, ref) => {
  return (
    <div ref={ref} data-side-panel className={`rounded-md bg-white/95 backdrop-blur-md shadow-lg border border-black/5 ${className}`}>
      {children}
    </div>
  )
})

FloatingCard.displayName = 'FloatingCard'
export default FloatingCard
