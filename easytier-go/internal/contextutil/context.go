package contextutil

import "context"

// WithoutCancel returns a copy of parent that is not canceled when parent is.
func WithoutCancel(parent context.Context) context.Context {
	return context.WithoutCancel(parent)
}

// AfterFunc arranges to call function in its own goroutine after context is
// canceled. The returned stop function reports whether it prevented the call.
func AfterFunc(ctx context.Context, function func()) func() bool {
	return context.AfterFunc(ctx, function)
}
