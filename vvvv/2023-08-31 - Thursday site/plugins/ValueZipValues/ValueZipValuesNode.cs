#region usings
using System;
using System.ComponentModel.Composition;

using VVVV.PluginInterfaces.V1;
using VVVV.PluginInterfaces.V2;
using VVVV.Utils.VColor;
using VVVV.Utils.VMath;

using VVVV.Core.Logging;

using System.Collections.Generic;
#endregion usings

namespace VVVV.Nodes
{
	#region PluginInfo
	[PluginInfo(Name = "ZipValues", Category = "Value", Help = "Basic template with one value in/out", Tags = "c#")]
	#endregion PluginInfo
	public class ValueZipValuesNode : IPluginEvaluate
	{
		#region fields & pins
		[Input("Positions")]
		public ISpread<ISpread<double>> FInPositions;
		
		[Input("Index")]
		public ISpread<int> FInIndex;

		[Output("Output")]
		public ISpread<ISpread<string>> FOutput;

		[Import()]
		public ILogger FLogger;
		#endregion fields & pins

		//called when data for any output pin is requested
		public void Evaluate(int SpreadMax)
		{
			var count = Math.Min(FInPositions.Count, FInIndex.Count);
			FOutput.SliceCount = count;
			for(int i=0; i<count; i++) {
				FOutput[i].SliceCount = 0;
				foreach(var input in FInPositions[i]) {
					FOutput[i].Add(input.ToString());
				}
				FOutput[i].Add(FInIndex[i].ToString());
			}
		}
	}
}
