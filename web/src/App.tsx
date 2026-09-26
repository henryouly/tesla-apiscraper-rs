import { Route, Router } from '@solidjs/router'
import { Layout } from './components/Layout'
import { AuthProvider, RequireAuth } from './lib/auth'
import { ThemeProvider } from './lib/theme'
import { CarIndex } from './pages/CarIndex'
import { SignIn } from './pages/SignIn'
import { CarSettings, ChargeCost, Geofences, Settings } from './pages/Stubs'

export function App() {
  return (
    <ThemeProvider>
      <AuthProvider>
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
          {/* Phase 7 stubs — guarded like the rest until they land */}
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
      </AuthProvider>
    </ThemeProvider>
  )
}
