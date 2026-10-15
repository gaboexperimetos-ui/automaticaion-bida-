---
name: Synthetic Cognition
colors:
  surface: '#101419'
  surface-dim: '#101419'
  surface-bright: '#36393f'
  surface-container-lowest: '#0a0e13'
  surface-container-low: '#181c21'
  surface-container: '#1c2025'
  surface-container-high: '#262a30'
  surface-container-highest: '#31353b'
  on-surface: '#e0e2ea'
  on-surface-variant: '#bbcac5'
  inverse-surface: '#e0e2ea'
  inverse-on-surface: '#2d3136'
  outline: '#859490'
  outline-variant: '#3c4946'
  surface-tint: '#4ddcc6'
  primary: '#b5fff0'
  on-primary: '#003730'
  primary-container: '#5eead4'
  on-primary-container: '#00675b'
  inverse-primary: '#006b5e'
  secondary: '#9ed753'
  on-secondary: '#203700'
  secondary-container: '#6b9f1f'
  on-secondary-container: '#1b2f00'
  tertiary: '#f5eeff'
  on-tertiary: '#381385'
  tertiary-container: '#dbcdff'
  on-tertiary-container: '#6448b2'
  error: '#ffb4ab'
  on-error: '#690005'
  error-container: '#93000a'
  on-error-container: '#ffdad6'
  primary-fixed: '#6ef9e2'
  primary-fixed-dim: '#4ddcc6'
  on-primary-fixed: '#00201b'
  on-primary-fixed-variant: '#005047'
  secondary-fixed: '#b9f46c'
  secondary-fixed-dim: '#9ed753'
  on-secondary-fixed: '#102000'
  on-secondary-fixed-variant: '#304f00'
  tertiary-fixed: '#e8ddff'
  tertiary-fixed-dim: '#cebdff'
  on-tertiary-fixed: '#21005e'
  on-tertiary-fixed-variant: '#4f319c'
  background: '#101419'
  on-background: '#e0e2ea'
  surface-variant: '#31353b'
typography:
  display:
    fontFamily: Manrope
    fontSize: 2.25rem
    fontWeight: '700'
    lineHeight: 2.75rem
    letterSpacing: -0.03em
  display-mobile:
    fontFamily: Manrope
    fontSize: 1.75rem
    fontWeight: '700'
    lineHeight: 2.25rem
    letterSpacing: -0.02em
  headline-lg:
    fontFamily: Manrope
    fontSize: 1.5rem
    fontWeight: '600'
    lineHeight: 2rem
    letterSpacing: -0.02em
  headline-md:
    fontFamily: Manrope
    fontSize: 1.25rem
    fontWeight: '600'
    lineHeight: 1.75rem
    letterSpacing: -0.015em
  headline-sm:
    fontFamily: Manrope
    fontSize: 1.125rem
    fontWeight: '600'
    lineHeight: 1.5rem
    letterSpacing: -0.01em
  body-lg:
    fontFamily: Hanken Grotesk
    fontSize: 1rem
    fontWeight: '400'
    lineHeight: 1.625rem
    letterSpacing: 0em
  body-md:
    fontFamily: Hanken Grotesk
    fontSize: 0.875rem
    fontWeight: '400'
    lineHeight: 1.375rem
    letterSpacing: 0.005em
  body-sm:
    fontFamily: Hanken Grotesk
    fontSize: 0.75rem
    fontWeight: '400'
    lineHeight: 1.125rem
    letterSpacing: 0.01em
  label-md:
    fontFamily: Hanken Grotesk
    fontSize: 0.875rem
    fontWeight: '500'
    lineHeight: 1.25rem
    letterSpacing: 0.01em
  label-sm:
    fontFamily: Hanken Grotesk
    fontSize: 0.75rem
    fontWeight: '600'
    lineHeight: 1rem
    letterSpacing: 0.04em
  code-inline:
    fontFamily: monospace
    fontSize: 0.8125rem
    fontWeight: '400'
    lineHeight: 1.25rem
    letterSpacing: 0em
rounded:
  sm: 0.25rem
  DEFAULT: 0.5rem
  md: 0.75rem
  lg: 1rem
  xl: 1.5rem
  full: 9999px
spacing:
  gutter: 1rem
  gutter-desktop: 1.5rem
  margin: 1rem
  margin-desktop: 2rem
  space-xs: 0.25rem
  space-sm: 0.5rem
  space-md: 0.75rem
  space-lg: 1.25rem
  space-xl: 2rem
---

## Brand & Style

This design system targets researchers, knowledge engineers, technical students, and power users constructing personal second brains. The interface resolves the tension between deep academic productivity and modern ambient intelligence, balancing intense utility with a subtle cyberpunk precision. 

The aesthetic avoids neon overload, adopting a calibrated, dark-room console ethos:
- **Tone:** Methodical, surgical, focused, highly cognitive.
- **Philosophy:** Low ocular fatigue over marathon study sessions, fast visual parsing, and semantic differentiation between human synthesis and generative intelligence.
- **Visual Stance:** Engineered dark glass and deep graphite slabs offset by high-luminance, fine-gauge neon telemetry. Borders are structural and crisp; overlays evoke high-end terminal software and technical instruments rather than consumer toy interfaces.

## Colors

The palette establishes an absolute visual hierarchy anchored in deep chromatic slate:

- **Canvas Base (`#0B0F14`):** The ground layer. Absorbs ambient light and maximizes contrast.
- **Primary Accent (`#5EEAD4` - Luminescent Cyan):** Reserved for generative AI telemetry, machine synthesis, semantic vector connections, and cognitive copilot states.
- **Secondary Accent (`#B8F36B` - Soft Lime):** Applied to user execution states, active academic goals, primary workflow progression, and actionable nodes.
- **Tertiary Accent (`#A78BFA` - Muted Violet):** Dedicated to taxonomic metadata, conceptual tags, temporal markers, and cross-reference links.
- **Surface Elevation:**
  - `surface-base`: `#111821` (Workspaces, editor panes, active canvases)
  - `surface-elevated`: `#17212B` (Floating panels, command palettes, context inspectors)
  - `surface-overlay`: `#1E2B38` (Dropdowns, floating pill badges, hover states)
- **Structural Outlines:** `#273442` (Precision dividers and borders with 1px hairline rendering).
- **Typography Contrast:**
  - `text-primary`: `#F1F5F9` (Headings, primary notes, critical readouts; exceeds 14:1 contrast on canvas).
  - `text-secondary`: `#94A3B8` (System metadata, secondary hierarchies, unselected states).
  - `text-muted`: `#64748B` (Line numbers, disabled states, structural cues).
- **Semantics:**
  - Success: `#34D399`
  - Warning: `#FBBF24`
  - Error: `#F87171`
  - Info: `#38BDF8`

## Typography

The typography implements a strict hierarchical separation:

- **Headlines (`Manrope`):** Delivers geometric stability with subtle technical modernism. Tight letter-spacing maintains compactness in information-dense dashboards and markdown headers.
- **Body & Controls (`Hanken Grotesk`):** Chosen for its exceptional legibility at small sizes and high-density rendering. Even letterforms prevent reading fatigue during intensive literature review and split-screen note-taking.
- **Scale Rules:** 
  - Document reading contexts default to `body-lg` (16px / 26px line-height) for prose balance.
  - Multi-panel productivity matrices (backlinks, graph nodes, file trees) default to `body-md` and `body-sm`.
  - Micro-telemetry, token counts, and file sizes utilize uppercase `label-sm` with tabular numerals.

## Layout & Spacing

This design system uses an adaptable, IDE-grade multi-pane layout model engineered for simultaneous document editing, knowledge graphing, and context retrieval:

- **Spatial Scale:** Built around a 4px/8px incremental grid. Components maintain a high-density footprint (`space-xs` and `space-sm`) for toolbars, inspectors, and file hierarchies, opening up to `space-lg` and `space-xl` for editor canvases.
- **Multi-Panel System:**
  - **Left Rail (Navigation/Vault):** Collapsible, fixed-range 240px–320px.
  - **Core Workspace:** Fluid container accommodating 1 to 3 split-view panes with 1px structural gutters.
  - **Right Context Rail (Inspector/AI Graph):** Dynamic sliding panel 320px–420px.
- **Breakpoints:**
  - **Mobile (<768px):** Single-pane view; contextual sidebars convert to full-height sheets; outer margins scale down to `1rem`.
  - **Tablet (768px–1024px):** Dual-pane view (Vault tree + Active document); inspector shifts to overlay.
  - **Desktop (>1024px):** Full tri-pane view unlocked; outer margins set to `2rem`, grid gutter set to `1.5rem`.

## Elevation & Depth

Depth is established through calibrated dark tonal layering and hairline luminescence rather than heavy drop shadows:

- **Tiered Surfaces:**
  - `Level 0 (Canvas)`: `#0B0F14` flat background.
  - `Level 1 (Slabs & Cards)`: `#111821` with a 1px solid border of `#273442`.
  - `Level 2 (Active Focus & Flyouts)`: `#17212B` with a 1px solid border of `#37485A`.
  - `Level 3 (Modals & Command Bar)`: `#1E2B38` with an ambient glow shadow: `0 16px 40px -12px rgba(0, 0, 0, 0.75), 0 0 0 1px rgba(94, 234, 212, 0.15)`.
- **Luminescent Edge Accents:** Active cards and focused editor blocks receive a subtle top or side gradient inset (`1px` width) drawing from `#5EEAD4` (AI) or `#B8F36B` (Human Action) at 40% opacity, providing visual feedback without shifting layout bounds.
- **Backdrop Filters:** Overlay panels, command palettes, and floating sticky toolbars use `backdrop-filter: blur(12px)` over `#111821` at 85% opacity.

## Shapes

The interface balances sharp instrument geometry with deliberate ergonomic curvature:

- **Level 2 Roundedness:**
  - Base interactive elements (inputs, buttons, list selections) strictly leverage `0.5rem` (8px).
  - Mid-tier surface blocks (standard cards, preview popovers, code blocks) utilize `rounded-lg` (1rem / 16px).
  - Large structural floating sheets, major modal dialogs, and workspace split containers feature `rounded-xl` (1.5rem / 24px).
- **Micro Elements:** Status pills, live indicators, and tag badges employ fully rounded pills to visually decouple metadata from rectangular workspace content.

## Components

### Buttons
- **Primary (Action):** Background `#B8F36B`, label `#0B0F14` (semi-bold `label-md`), `roundedness: 2`. On hover, subtle luminance boost with `box-shadow: 0 0 16px rgba(184, 243, 107, 0.35)`.
- **AI Assist:** Background `rgba(94, 234, 212, 0.08)`, border `1px solid #5EEAD4`, label `#5EEAD4`. Hover elevates background to `rgba(94, 234, 212, 0.16)` with subtle cyan peripheral glow.
- **Ghost/Tertiary:** Background transparent, border transparent, label `#94A3B8`. Hover yields `#17212B` background with `#F1F5F9` text.

### Chips & Badges
- **Semantic Tag:** Height 24px, radius pill, background `rgba(167, 139, 250, 0.12)`, border `1px solid rgba(167, 139, 250, 0.3)`, text `#A78BFA` (`label-sm`).
- **AI Inference Chip:** Height 24px, radius pill, background `rgba(94, 234, 212, 0.1)`, border `1px solid rgba(94, 234, 212, 0.35)`, text `#5EEAD4`. Prefixed with a 6px glowing status dot.

### Input Fields & Command Palettes
- **Text Inputs:** Height 40px, background `#111821`, border `1px solid #273442`, radius 8px, text `#F1F5F9`, placeholder `#64748B`.
- **Focus State:** Border transitions to `#5EEAD4` with `0 0 0 1px #5EEAD4` ring; zero layout shift.
- **Command Palette (Cmd+K):** Centered floating overlay, `rounded-xl`, background `#17212B`, subtle border `1px solid rgba(94, 234, 212, 0.2)`. Fast-filter list items highlight with `#1E2B38` and an active indicator bar in `#B8F36B`.

### Cards & Knowledge Blocks
- **Note/Document Card:** Background `#111821`, border `1px solid #273442`, padding `space-lg`, `rounded-lg`.
- **Active / Dragging State:** Border color `#5EEAD4`, elevated with `Level 2` depth.
- **Reference Card (Second Brain Backlink):** Inset background `#0B0F14`, left border `2px solid #A78BFA`, padding `space-md`, typography in `body-sm`.

### Checkboxes & Switches
- **Checkboxes:** Size 18px x 18px, background `#111821`, border `1px solid #273442`, radius 4px. Checked state transitions to `#B8F36B` fill with `#0B0F14` checkmark.
- **AI Auto-Sync Switch:** Track size 36px x 20px, radius pill. Off-state `#273442`; On-state `#5EEAD4` track with `#0B0F14` sliding thumb.

### Graph & Document Nodes (Domain Specific)
- **Interactive Knowledge Node:** Small radial nodes; user notes rendered in `#F1F5F9` with `#B8F36B` connections; AI-synthesized conceptual bridges rendered in glowing `#5EEAD4` dashed vectors. Hover state triggers a floating contextual snippet card with a 12px blur backdrop.