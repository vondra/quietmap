// The floating card every panel of the map sits in, on the theme's card colour.
import { forwardRef } from 'react'

interface FloatingCardProps {
  children: React.ReactNode
  className?: string
}

const FloatingCard = forwardRef<HTMLDivElement, FloatingCardProps>(({ children, className = '' }, ref) => {
  return (
    <div ref={ref} data-side-panel className={`rounded-md bg-card/95 backdrop-blur-md shadow-lg border border-border ${className}`}>
      {children}
    </div>
  )
})

FloatingCard.displayName = 'FloatingCard'
export default FloatingCard
