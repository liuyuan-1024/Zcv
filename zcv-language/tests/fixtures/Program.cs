using System;
using System.Collections.Generic;

namespace HighlightingSample;

public record Config(bool Enabled, int Retries);

public class Program
{
    private static string Greeting(string name) => $"Hello, {name}!";

    public static int Main()
    {
        var config = new Config(true, 3);
        var names = new List<string> { "Zcv", "C#" };
        foreach (var name in names)
            if (config.Enabled) Console.WriteLine(Greeting(name));
        return config.Retries;
    }
}
