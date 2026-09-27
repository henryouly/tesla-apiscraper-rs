import { Route, Router } from '@solidjs/router'
import { Layout } from './components/Layout'
import { AuthProvider, RequireAuth } from './lib/auth'
import { ThemeProvider } from './lib/theme'
import { UnitsProvider } from './lib/units'
import { CarIndex } from './pages/CarIndex'
import { CarSettings } from './pages/CarSettings'
import { ChargeCost } from './pages/ChargeCost'
import { Geofences } from './pages/Geofences'
import { Settings } from './pages/Settings'
import { SignIn } from './pages/SignIn'

export function App() {
  return (
    <ThemeProvider>
      <AuthProvider>
        <UnitsProvider>
        <Router root={Layout}>
          <Route path="/signin" component={SignIn} />
          <Route
            path="/"
            component={() => (
              <RequireAuth>
                <CarIndex />
              </RequireAuth>
            )}
          />
          {/* All routes are implemented; auth-guarded */}
          <Route
            path="/settings"
            component={() => (
              <RequireAuth>
                <Settings />
              </RequireAuth>
            )}
          />
          <Route
            path="/settings/car/:id"
            component={() => (
              <RequireAuth>
                <CarSettings />
              </RequireAuth>
            )}
          />
          <Route
            path="/geofences"
            component={() => (
              <RequireAuth>
                <Geofences />
              </RequireAuth>
            )}
          />
          <Route
            path="/charge/:id/cost"
            component={() => (
              <RequireAuth>
                <ChargeCost />
              </RequireAuth>
            )}
          />
        </Router>
        </UnitsProvider>
      </AuthProvider>
    </ThemeProvider>
  )
}
